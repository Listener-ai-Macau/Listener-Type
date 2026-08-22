//! Volcengine SAUC bigmodel streaming ASR client.
//!
//! Direct port of the Swift `VolcengineStreamingASR`. Battle-tested protocol
//! quirks are preserved verbatim — see comments tagged with `[asr]` for the
//! original learnings (especially the "definite=true is NOT stream end" bug).

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex as ParkingMutex;
use serde_json::{json, Value};
use tokio::net::TcpStream;
use tokio::runtime::Handle;
use tokio::sync::{mpsc, oneshot, Mutex as AsyncMutex, Notify, OnceCell};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::HeaderValue;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};
use uuid::Uuid;

use super::frame::{self, Flags, MessageType, Serialization};
use super::{AudioConsumer, DictionaryHotword, RawTranscript};

const FINAL_TRANSCRIPT_ENDPOINT: &str = "wss://openspeech.bytedance.com/api/v3/sauc/bigmodel_async";
const BIDIRECTIONAL_TRANSCRIPT_ENDPOINT: &str =
    "wss://openspeech.bytedance.com/api/v3/sauc/bigmodel";
/// 100 ms of 16 kHz / 16-bit / mono PCM.
const TARGET_AUDIO_CHUNK_BYTES: usize = 3_200;
/// 16 kHz · 16-bit · mono = 32 000 bytes/sec → 32 bytes/ms.
const BYTES_PER_MS: f64 = 32.0;
const HOTWORD_CAP: usize = 80;
const FINAL_RESULT_TIMEOUT: Duration = Duration::from_secs(12);
const FINAL_PARTIAL_COVERAGE_SLACK_MS: u64 = 600;
const WEBSOCKET_SEND_TIMEOUT: Duration = Duration::from_millis(1_200);
// F2 云端空转（2026-08-09 12:47:04：963 包全收、云端 audio_duration 长到
// 10210ms 但 result_chars 停在 2）——终稿空但本地整段持续人声时允许一次留存
// 重试的判定阈：本地音频至少 1.5s 且人声尾端正点距音频末尾不超过 0.5s。
const EMPTY_SPIN_MIN_LOCAL_AUDIO_MS: u64 = 1_500;
const LOCAL_SHADOW_OWNER_END_ALIGNMENT_MS: u64 = 600;
const EMPTY_SPIN_SPEECH_TAIL_SLACK_MS: u64 = 500;
// A short physical-key dictation can contain real but very low-energy speech
// which Volcengine rejects twice when replayed byte-for-byte. The normal live
// path remains firmware-levelled and untouched. Only the single empty-final
// recovery representation may use this bounded +9 dB lift, with the same
// -3 dBFS peak ceiling used by the host safety limiter.
const EMPTY_FINAL_REPLAY_MAX_GAIN: f64 = 2.818_382_931;
const EMPTY_FINAL_REPLAY_PEAK_CEILING: f64 = i16::MAX as f64 * 0.707_945_784;
// 建连是全文件唯一曾经无超时边界的网络操作：TCP 黑洞下会挂到 OS 级超时
// （21s+），期间 stop/cancel 只能排队，proactive 末帧预算耗尽后丢稿。
const WEBSOCKET_CONNECT_TIMEOUT: Duration = Duration::from_secs(6);
const FINAL_FRAME_SEND_BUDGET: Duration = Duration::from_millis(1_800);
const FINAL_AUDIO_DRAIN_MIN_BUDGET: Duration = Duration::from_millis(800);
const FINAL_AUDIO_DRAIN_MAX_BUDGET: Duration = Duration::from_secs(8);
const AUDIO_FRAME_DURATION_MS: u64 = 100;
const SECOND_PASS_END_WINDOW_MS: u32 = 500;
// Listener sends an explicit stop from the hardware. Do not let a brief pause
// after the record press make the provider classify the whole session as empty.
const SECOND_PASS_FORCE_TO_SPEECH_MS: u32 = 0;

#[derive(Clone, Debug)]
pub struct VolcengineCredentials {
    pub app_id: String,
    pub access_token: String,
    pub resource_id: String,
}

impl VolcengineCredentials {
    pub fn default_resource_id() -> &'static str {
        "volc.seedasr.sauc.duration"
    }
}

#[derive(Clone, Debug, thiserror::Error)]
pub enum VolcengineASRError {
    #[error("credentials missing")]
    CredentialsMissing,
    #[error("connection failed: {0}")]
    ConnectionFailed(String),
    /// WebSocket 握手阶段服务端返回 401 / 403：凭据被拒。
    /// 区分自 `ConnectionFailed`（DNS/TLS/网络层失败）—— 前者通常是 App ID / Access
    /// Token / Resource ID 错或账号没开通 bigmodel；后者是网络断 / 防火墙 / DNS。
    /// 文案简短，原因在文档里说明，capsule 不堆长引导。
    #[error("凭据被拒（{0}）")]
    AuthRejected(u16),
    #[error("authentication failed")]
    AuthenticationFailed,
    #[error("识别额度已用完（错误码 {0}）；请在火山引擎补充 ASR 时长，或在设置中切换识别服务")]
    QuotaExceeded(u32),
    #[error("no final result")]
    NoFinalResult,
    #[error("final result timed out")]
    FinalResultTimeout,
    #[error(
        "final transcript did not cover sent audio after bounded provider grace (sent_audio_ms={sent_audio_ms}, transcript_end_ms={transcript_end_ms})"
    )]
    FinalResultCoverageIncomplete {
        sent_audio_ms: u64,
        transcript_end_ms: u64,
    },
    #[error("decode failed: {0}")]
    DecodeFailed(String),
}

impl VolcengineASRError {
    pub fn permits_full_audio_replay(&self) -> bool {
        matches!(
            self,
            Self::ConnectionFailed(_)
                | Self::NoFinalResult
                | Self::FinalResultTimeout
                | Self::FinalResultCoverageIncomplete { .. }
        )
    }
}

const VOLCENGINE_AUDIO_DURATION_QUOTA_EXCEEDED: u32 = 45_000_292;

fn classify_provider_error(code: u32, body: &str) -> VolcengineASRError {
    if code == VOLCENGINE_AUDIO_DURATION_QUOTA_EXCEEDED
        || body.to_ascii_lowercase().contains("quota exceeded")
    {
        return VolcengineASRError::QuotaExceeded(code);
    }
    VolcengineASRError::ConnectionFailed(format!("ASR error {code}: {body}"))
}

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

fn bounded_empty_final_replay_pcm(pcm: &[u8]) -> (Vec<u8>, f64, u16, u16) {
    let peak_before = pcm
        .chunks_exact(2)
        .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]).unsigned_abs())
        .max()
        .unwrap_or(0);
    if peak_before == 0 {
        return (pcm.to_vec(), 1.0, 0, 0);
    }
    let peak_limited_gain = EMPTY_FINAL_REPLAY_PEAK_CEILING / f64::from(peak_before);
    let gain = EMPTY_FINAL_REPLAY_MAX_GAIN.min(peak_limited_gain);
    if gain <= 1.0 {
        return (pcm.to_vec(), 1.0, peak_before, peak_before);
    }

    let mut recovered = Vec::with_capacity(pcm.len());
    for chunk in pcm.chunks_exact(2) {
        let sample = i16::from_le_bytes([chunk[0], chunk[1]]);
        let scaled = (f64::from(sample) * gain)
            .round()
            .clamp(i16::MIN as f64, i16::MAX as f64) as i16;
        recovered.extend_from_slice(&scaled.to_le_bytes());
    }
    recovered.extend_from_slice(pcm.chunks_exact(2).remainder());
    let peak_after = recovered
        .chunks_exact(2)
        .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]).unsigned_abs())
        .max()
        .unwrap_or(0);
    (recovered, gain, peak_before, peak_after)
}
type WsSink = futures_util::stream::SplitSink<WsStream, Message>;
type SharedWriter = Arc<AsyncMutex<Option<WsSink>>>;
type PartialTranscriptCallback = Arc<dyn Fn(String) + Send + Sync>;
type VisualPartialTranscriptCallback = Arc<dyn Fn(String) + Send + Sync>;
type FinalIntermediateTranscriptCallback = Arc<dyn Fn(FinalIntermediateTranscript) + Send + Sync>;
type TargetSpeakerUpdateCallback = Arc<dyn Fn(TargetSpeakerUpdate) + Send + Sync>;
type StreamingEventCallback = Arc<dyn Fn(VolcengineStreamingEvent) + Send + Sync>;

#[derive(Clone, Debug)]
pub struct FinalIntermediateTranscript {
    pub text: String,
    pub authoritative_two_pass: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetSpeakerUpdate {
    pub speaker_id: Option<String>,
    pub target_speech_end_ms: Option<u64>,
    /// Furthest audio boundary covered by an authoritative provider response.
    /// This intentionally stays separate from the locally captured duration so
    /// endpointing cannot outrun a late speaker-attributed sentence tail.
    pub provider_audio_duration_ms: Option<u64>,
    pub audio_duration_ms: Option<u64>,
    pub local_speech_end_ms: Option<u64>,
    pub local_target_speech_end_ms: Option<u64>,
    pub local_non_target_speech_end_ms: Option<u64>,
    pub local_speaker_tracking_enabled: bool,
    pub stable_attributed_speech_end_ms: Option<u64>,
    pub target_activity_advanced: bool,
    pub pending_unattributed_speech: bool,
    pub pending_activity_advanced: bool,
    pub speaker_info_present: bool,
}

/// Events exposed to the platform adapter without changing the legacy
/// product preview callbacks. The product callback intentionally also emits
/// the final preview; the adapter needs the protocol-level distinction.
#[derive(Clone, Debug)]
pub(crate) enum VolcengineStreamingEvent {
    Partial(String),
    Final(String),
    Error(VolcengineASRError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VolcengineSessionEndpoint {
    OptimizedBidirectional,
    Bidirectional,
}

impl VolcengineSessionEndpoint {
    pub fn endpoint(self) -> &'static str {
        match self {
            Self::OptimizedBidirectional => FINAL_TRANSCRIPT_ENDPOINT,
            Self::Bidirectional => BIDIRECTIONAL_TRANSCRIPT_ENDPOINT,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::OptimizedBidirectional => "authoritative_bidirectional",
            Self::Bidirectional => "authoritative_realtime",
        }
    }

    fn emits_stream_preview_before_final(self) -> bool {
        // The optimized endpoint returns low-latency `stream` hypotheses and
        // later `two_pass` corrections on the same provider session. Hiding
        // the former delays the capsule until the correction arrives.
        match self {
            Self::OptimizedBidirectional | Self::Bidirectional => true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VolcengineResultType {
    Full,
    Single,
}

impl VolcengineResultType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Single => "single",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VolcengineSessionOptions {
    pub endpoint: VolcengineSessionEndpoint,
    pub enable_nonstream: bool,
    pub result_type: VolcengineResultType,
    pub end_window_size_ms: Option<u32>,
    pub force_to_speech_time_ms: Option<u32>,
}

impl Default for VolcengineSessionOptions {
    fn default() -> Self {
        Self {
            endpoint: VolcengineSessionEndpoint::OptimizedBidirectional,
            enable_nonstream: true,
            result_type: VolcengineResultType::Full,
            end_window_size_ms: Some(SECOND_PASS_END_WINDOW_MS),
            force_to_speech_time_ms: Some(SECOND_PASS_FORCE_TO_SPEECH_MS),
        }
    }
}

#[derive(Clone, Debug)]
enum AudioDeliveryReadiness {
    Opening,
    Ready,
    Failed(VolcengineASRError),
    Cancelled,
}

impl Default for AudioDeliveryReadiness {
    fn default() -> Self {
        // The producer is returned before the async WebSocket opener runs.
        // Treat that interval as opening so a very short recording can wait
        // for the real outcome instead of sending a final frame to no writer.
        Self::Opening
    }
}

use super::volcengine_transcript::{
    authoritative_final_supersedes_repeated_streaming_ledger, is_unstable_initial_partial,
    normalize_cjk_final_spacing_and_echoes, normalized_result, transcript_candidate_from_result,
    trim_repeated_short_final_tail, trim_repeated_short_streaming_tail, TranscriptSegment,
};
use super::volcengine_untimed_merge::merge_streaming_candidate_with_untimed_window;

/// Sync state shared across the receive loop, the public API, and the
/// audio-consumer fast path.
#[derive(Default)]
struct SyncState {
    pending_audio: Vec<u8>,
    next_sequence: i32,
    bytes_sent: usize,
    frames_sent: usize,
    response_frames_seen: usize,
    partial_updates_seen: usize,
    is_connected: bool,
    final_tx: Option<oneshot::Sender<Result<RawTranscript, VolcengineASRError>>>,
    runtime: Option<Handle>,
    start: Option<Instant>,
    finishing: bool,
    audio_delivery_readiness: AudioDeliveryReadiness,
    /// 最近一次 partial（非 final）的累积 transcript。服务端在 final 帧到达前
    /// 关闭连接 / 网络中断时，作为 fallback 回给上层，避免「用户的话已经识别出来
    /// 但没拿到 final」就丢光。
    last_partial_text: String,
    /// 服务端长流有时会把 result.text 从累计全文重置为最近片段。这里保留客户端
    /// 侧合并后的最长上下文，final 帧只返回尾段时也不丢前文。
    best_transcript_text: String,
    /// 火山流式响应会反复发送同一 utterance 起点的修订文本。用时间戳合并这些片段，
    /// 避免把同一段尾巴当作新内容追加，导致胶囊预览和最终插入重复膨胀。
    best_transcript_segments: Vec<TranscriptSegment>,
    best_untimed_window: String,
    /// Speaker attribution lags behind the provider's streaming text. Keep a
    /// display-only merge here so the capsule can advance without promoting
    /// provisional words into the authoritative/final transcript.
    optimistic_preview_text: String,
    optimistic_preview_segments: Vec<TranscriptSegment>,
    optimistic_untimed_window: String,
    last_emitted_preview_text: String,
    /// Capsule-only provisional text. This ledger is intentionally separate
    /// from every authoritative/optimistic transcript ledger so a visual
    /// cadence improvement cannot leak into endpointing or final insertion.
    last_emitted_visual_preview_text: String,
    /// 最新服务端响应已处理的音频时长。two-pass 终帧可能只给最后一个 utterance 的
    /// 词时间戳，但 `audio_info.duration` 仍覆盖整段音频；收尾时应优先采用这项
    /// 传输级覆盖证据，避免把已返回的完整文本误判为截断。
    last_server_audio_duration_ms: Option<u64>,
    target_speaker_id: Option<String>,
    target_speech_end_ms: Option<u64>,
    /// Stable end of the physical wake row. Unlike `target_speech_end_ms`,
    /// this never advances with dictation and can safely anchor an immediate
    /// provider speaker-cluster split across later streaming responses.
    wake_target_speech_end_ms: Option<u64>,
    stable_attributed_speech_end_ms: Option<u64>,
    local_audio_duration_ms: Option<u64>,
    local_speech_end_ms: Option<u64>,
    local_target_speech_end_ms: Option<u64>,
    local_non_target_speech_end_ms: Option<u64>,
    /// Endpoint-grade owner-absence boundary. This advances only after a
    /// low-score run has persisted long enough to distinguish a continuing
    /// second speaker from the real owner's two-window cross-phrase dip.
    local_sustained_non_target_speech_end_ms: Option<u64>,
    local_speaker_classification: Option<crate::speaker_verification::SessionSpeakerClassification>,
    local_speaker_tracking_enabled: bool,
    /// The automatic wake gate already matched the persisted owner voiceprint.
    /// Keep this separate from `local_target_confirmed`: the former identifies
    /// the wake speaker, while the latter requires a positive body-window match
    /// and remains the stricter authority for refreshing the owner endpoint.
    local_wake_owner_verified: bool,
    /// True when the session identity was inferred from the short wake audio
    /// instead of a user-enrolled voiceprint. This profile is useful as a hint,
    /// but is not reliable enough to erase provider-recognized dictation.
    local_speaker_profile_adaptive: bool,
    local_speaker_stable_target: bool,
    local_target_confirmed: bool,
    local_consecutive_target: u8,
    local_consecutive_non_target: u8,
    /// Independent transcript-only evidence counter. It must never reuse the
    /// endpoint identity counters. Even two extreme windows only stage a
    /// checkpoint: a mixed owner+room window can score as a severe mismatch,
    /// so destructive rollback waits for a stable provider utterance boundary
    /// that agrees with the local owner-absence timeline.
    local_consecutive_transcript_hard_non_target: u8,
    /// Session voiceprint negatives are advisory for transcript ownership. Two
    /// very-low-score windows may still mark current speech as another person
    /// for endpointing, but must never freeze/delete provider-recognized text.
    local_consecutive_strong_non_target: u8,
    local_owner_absence_run_started_ms: Option<u64>,
    local_owner_absence_run_confirmed: bool,
    local_speaker_evidence: Vec<LocalSpeakerEvidence>,
    wake_speaker_phrase: Option<String>,
    speaker_info_present: bool,
    pending_unattributed_text: String,
    /// Hard multi-speaker isolation: provider/local dual-gate evidence or two
    /// enrolled-owner extreme mismatches freeze the owner ledger so later
    /// polluted cloud finals / optimistic growth cannot re-expand room speech
    /// into the insert path. Cleared only after fresh Target evidence.
    owner_isolation_frozen: bool,
    owner_isolation_ceiling_text: String,
    owner_isolation_ceiling_segments: Vec<TranscriptSegment>,
}

#[derive(Clone, Debug)]
struct PendingSessionSpeakerAnchor {
    tracking_enabled: bool,
    wake_owner_verified: bool,
    profile_adaptive: bool,
    wake_phrase: Option<String>,
}

impl PendingSessionSpeakerAnchor {
    fn capture(state: &SyncState) -> Self {
        Self {
            tracking_enabled: state.local_speaker_tracking_enabled,
            wake_owner_verified: state.local_wake_owner_verified,
            profile_adaptive: state.local_speaker_profile_adaptive,
            wake_phrase: state.wake_speaker_phrase.clone(),
        }
    }

    fn restore_after_stream_reset(self, state: &mut SyncState) {
        state.local_speaker_tracking_enabled = self.tracking_enabled;
        state.local_wake_owner_verified = self.tracking_enabled && self.wake_owner_verified;
        state.local_speaker_profile_adaptive = self.tracking_enabled && self.profile_adaptive;
        state.local_speaker_stable_target = self.tracking_enabled;
        state.wake_speaker_phrase = self.wake_phrase;
    }
}

#[derive(Clone, Copy, Debug)]
struct LocalSpeakerEvidence {
    audio_end_ms: u64,
    classification: crate::speaker_verification::SessionSpeakerClassification,
    stable_target: bool,
}

const LOCAL_SPEAKER_SWITCH_CONFIRMATIONS: u8 = 2;
const LOCAL_SPEAKER_EVIDENCE_LIMIT: usize = 64;
const LOCAL_SPEAKER_WINDOW_MS: u64 = 1_200;
const LOCAL_ENDPOINT_STRONG_NON_TARGET_MAX_SCORE: f32 = 0.20;
// Transcript isolation is deliberately stricter than endpointing. Only an
// extreme mismatch may freeze future transcript growth; ordinary same-owner
// cross-phrase dips (observed around 0.17) remain advisory.
const LOCAL_TRANSCRIPT_HARD_NON_TARGET_MAX_SCORE: f32 = 0.10;
const LOCAL_ENDPOINT_OWNER_ABSENCE_CONTINUATION_MAX_SCORE: f32 = 0.30;
// Three 400 ms overlapping low-score observations are enough to distinguish
// sustained other speech from the two-window startup dip seen in session 2024.
const LOCAL_ENDPOINT_OWNER_ABSENCE_MIN_RUN_MS: u64 = 800;
// A cloud speaker id can collapse two real people into the same cluster. Do
// not trust that id for a later stable utterance when the local verifier saw a
// sustained owner-absence run over the whole utterance. Keep this deliberately
// above the hard NonTarget threshold: field session d96a8653 had six windows
// at 0.17..0.30 for the second person, while the accepted same-owner low-band
// regression stayed at 0.307..0.337. Three windows (about 1.2 s with the
// current cadence) prevents a single cross-phrase false negative from deleting
// owner speech.
const LOCAL_OWNER_ABSENCE_MAX_SCORE: f32 = 0.30;
const LOCAL_OWNER_ABSENCE_CONFIRMATIONS: u32 = 3;
const MAX_WAKE_PHRASE_UTTERANCE_MS: u64 = 1_800;
const MAX_WAKE_BODY_CLUSTER_SPLIT_GAP_MS: u64 = 500;
// Volcengine's two-pass diarization can place adjacent same-speaker clusters a
// little on top of each other even when the PCM contains a clean pause. Field
// session 1705 kept the verified owner locally but dropped the whole second
// clause because the strict `next_start >= previous_end` check treated that
// timestamp jitter as simultaneous speech. Only an enrolled, wake-verified
// owner with a positively confirmed body may use this tolerance; explicit
// local NonTarget evidence and larger/real overlaps remain fail-closed.
const MAX_VERIFIED_OWNER_CLUSTER_HANDOFF_OVERLAP_MS: u64 = 200;
// Volcengine session 768 emitted an exact wake-only row spanning 1,902 ms.
// Permit that provider timing drift only for unenrolled/adaptive final recovery;
// the normal speaker-attribution path keeps the stricter 1,800 ms boundary.
const MAX_ADAPTIVE_WAKE_ONLY_UTTERANCE_MS: u64 = 2_500;

fn latest_audio_duration_ms(state: &SyncState) -> Option<u64> {
    state
        .last_server_audio_duration_ms
        .into_iter()
        .chain(state.local_audio_duration_ms)
        .max()
}

fn target_speaker_update_from_state(
    state: &SyncState,
    target_activity_advanced: bool,
    pending_activity_advanced: bool,
) -> TargetSpeakerUpdate {
    TargetSpeakerUpdate {
        speaker_id: state.target_speaker_id.clone(),
        target_speech_end_ms: state.target_speech_end_ms,
        provider_audio_duration_ms: state.last_server_audio_duration_ms,
        audio_duration_ms: latest_audio_duration_ms(state),
        local_speech_end_ms: state.local_speech_end_ms,
        local_target_speech_end_ms: state.local_target_speech_end_ms,
        // Keep transcript filtering's immediate advisory boundary private.
        // Endpointing sees only the sustained owner-absence boundary so the
        // installed session-2024 two-window owner dip cannot stop recording.
        local_non_target_speech_end_ms: state.local_sustained_non_target_speech_end_ms,
        local_speaker_tracking_enabled: state.local_speaker_tracking_enabled,
        stable_attributed_speech_end_ms: state.stable_attributed_speech_end_ms,
        target_activity_advanced,
        pending_unattributed_speech: !state.pending_unattributed_text.is_empty(),
        pending_activity_advanced,
        speaker_info_present: state.speaker_info_present,
    }
}

/// While the newest local sample is still Target, growing owner ASR activity
/// (Target-filtered / Target-gated provisional preview) may refresh the owner
/// endpoint clock. Mid-sentence Uncertain/NonTarget dips must NOT refresh from
/// text growth: other-speaker provisional text used to ride this path during
/// identity debounce and keep auto-end open while the room spoke (installed
/// session baeff75a: filtered ~26 chars, optimistic grew to 115, final pasted
/// the mixed room text).
///
/// An Uncertain local window does not refresh the endpoint clock. If the owner
/// is still speaking, Target-gated preview growth below provides the protected
/// continuation signal without letting another speaker's weak match hold the
/// recording open.
fn refresh_local_target_from_owner_preview_activity(state: &mut SyncState) -> bool {
    if !state.local_target_confirmed || !local_speaker_allows_owner_endpoint_refresh(state) {
        return false;
    }
    let Some(audio_ms) = latest_audio_duration_ms(state) else {
        return false;
    };
    let previous = state.local_target_speech_end_ms.unwrap_or_default();
    if audio_ms <= previous {
        return false;
    }
    state.local_target_speech_end_ms = Some(audio_ms);
    true
}

fn refresh_local_target_from_owner_safe_provider_split(state: &mut SyncState) -> bool {
    // A sequential cloud split is sufficient to recover/display text, but it
    // is not independent proof that the latest sound still belongs to the
    // owner. Reuse the strict Target-only refresh gate so an Uncertain room
    // voice cannot keep auto-end alive through provider preview growth.
    refresh_local_target_from_owner_preview_activity(state)
}

fn local_speaker_allows_optimistic_preview(state: &SyncState) -> bool {
    if !state.local_speaker_tracking_enabled {
        return true;
    }
    if !state.local_speaker_stable_target {
        return false;
    }
    // `stable_target` is the debounced identity decision. A single enrolled
    // voiceprint window often falls from Target to Uncertain as the user moves
    // from the short wake phrase into natural speech; requiring every window to
    // remain Target hid valid provider text for the rest of that utterance.
    // Keep explicit NonTarget fail-closed, but let Uncertain render while the
    // debounced identity is still the owner. Endpoint refresh remains stricter
    // below, so an uncertain preview cannot keep a room-speech session alive.
    !matches!(
        state.local_speaker_classification.as_ref(),
        Some(crate::speaker_verification::SessionSpeakerClassification::NonTarget { .. })
    )
}

fn display_only_provisional_preview_candidate(
    state: &mut SyncState,
    provider_result: &Value,
    pending_unattributed_speech: bool,
) -> Option<String> {
    if !pending_unattributed_speech
        || !state.local_speaker_tracking_enabled
        || !state.local_wake_owner_verified
        || !state.local_speaker_stable_target
        || state.owner_isolation_frozen
        || state.local_owner_absence_run_confirmed
        || state.local_non_target_speech_end_ms.is_some()
        || matches!(
            state.local_speaker_classification.as_ref(),
            Some(crate::speaker_verification::SessionSpeakerClassification::NonTarget { .. })
        )
    {
        return None;
    }
    let candidate = provider_result
        .get("text")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())?;
    let authoritative_len = spoken_content_len(&state.last_emitted_preview_text);
    let visual_len = spoken_content_len(&state.last_emitted_visual_preview_text);
    let candidate_len = spoken_content_len(candidate);
    if candidate_len <= authoritative_len.max(visual_len)
        || candidate == state.last_emitted_visual_preview_text
    {
        return None;
    }
    state.last_emitted_visual_preview_text = candidate.to_string();
    Some(candidate.to_string())
}

fn local_speaker_allows_owner_endpoint_refresh(state: &SyncState) -> bool {
    state.local_speaker_stable_target
        && matches!(
            state.local_speaker_classification.as_ref(),
            Some(crate::speaker_verification::SessionSpeakerClassification::Target { .. })
        )
}

/// Session voiceprint negatives are not reliable enough across phrases to
/// destroy a final transcript. They may suppress live preview and contribute to
/// endpointing, while provider diarization remains the authority for excluding
/// a second speaker from the committed result.
fn local_speaker_allows_non_destructive_final_recovery(state: &SyncState) -> bool {
    !state.local_speaker_tracking_enabled
        || (state.local_speaker_stable_target
            && !state.owner_isolation_frozen
            && state.local_consecutive_non_target == 0)
}

/// A non-final two-pass response can temporarily contain fewer attributed
/// utterances than the already displayed owner-safe stream. Session 1868 grew
/// to 45 provider chars, regressed to 34 while a third utterance was pending,
/// then recovered to 46 at protocol final. Keep the capsule monotonic through
/// that attribution gap. A confirmed speaker switch freezes the owner ledger
/// and deliberately disables this protection so foreign speech can be removed.
fn should_preserve_longer_owner_preview(
    state: &SyncState,
    candidate: &str,
    has_final: bool,
) -> bool {
    !has_final
        && state.local_speaker_tracking_enabled
        && state.local_speaker_stable_target
        && state.local_target_confirmed
        && !state.owner_isolation_frozen
        && !state.last_emitted_preview_text.trim().is_empty()
        && spoken_content_len(candidate) < spoken_content_len(&state.last_emitted_preview_text)
}

fn final_wake_only_provider_gap_is_owner_safe(
    state: &SyncState,
    provider_result: &Value,
    target_text: &str,
) -> bool {
    let provider_text = provider_result
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default();
    // Final speaker filtering can return an empty target even though the
    // session ledger already committed the wake row (session 768). Use that
    // accepted ledger only as a wake-only recovery anchor, never as arbitrary
    // body text.
    let recovery_target_text = if target_text.trim().is_empty() {
        state.best_transcript_text.as_str()
    } else {
        target_text
    };
    let local_identity_allows_recovery = state.local_speaker_profile_adaptive
        || (state.local_speaker_stable_target
            && !state.owner_isolation_frozen
            && state.local_consecutive_non_target == 0);
    if !state.local_speaker_tracking_enabled
        || !local_identity_allows_recovery
        || spoken_content_len(provider_text) <= spoken_content_len(recovery_target_text)
    {
        return false;
    }
    let Some(wake_phrase) = state.wake_speaker_phrase.as_deref() else {
        return false;
    };
    let normalized_phrase = wake_phrase
        .chars()
        .filter(|ch| ch.is_alphanumeric())
        .collect::<String>();
    let normalized_target = recovery_target_text
        .chars()
        .filter(|ch| ch.is_alphanumeric())
        .collect::<String>();
    if normalized_phrase.is_empty() || normalized_target != normalized_phrase {
        return false;
    }
    let normalized_provider = provider_text
        .chars()
        .filter(|ch| ch.is_alphanumeric())
        .collect::<String>();
    if !normalized_provider.starts_with(&normalized_phrase)
        || normalized_provider.len() <= normalized_phrase.len()
    {
        return false;
    }

    // Field evidence 2026-08-10: Volcengine's final `result.text` contained
    // the complete owner dictation, but `utterances` regressed to only the
    // bounded "开始录音" row. Recover only this exact provider-schema gap. In
    // addition to one short wake row and an unchanged local identity, require
    // the provider to have previously advanced stable attribution beyond that
    // wake row. This is stronger evidence than the display-preview ledger:
    // Uncertain local windows intentionally prevent preview admission, while
    // the provider can still have already attributed the body to the owner.
    // A second stable utterance/speaker or any active NonTarget switch keeps
    // enrolled-voice isolation fail-closed. For an unenrolled session, the
    // short wake-derived profile is advisory: real sessions 81/86 showed it
    // classifying the same user's body as NonTarget. In this exact provider
    // wake-only schema gap, prefer recognized user text over a false empty.
    let stable_utterances = provider_result
        .get("utterances")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|utterance| utterance_is_stable(utterance))
        .collect::<Vec<_>>();
    if stable_utterances.len() != 1
        || !stable_utterances[0]
            .get("text")
            .and_then(Value::as_str)
            .map(|text| {
                text.chars()
                    .filter(|ch| ch.is_alphanumeric())
                    .collect::<String>()
                    == normalized_phrase
            })
            .unwrap_or(false)
    {
        return false;
    }

    let (Some(wake_start_ms), Some(wake_end_ms)) = (
        utterance_start_ms(stable_utterances[0]),
        utterance_end_ms(stable_utterances[0]),
    ) else {
        return false;
    };
    if state.local_speaker_profile_adaptive {
        // Installed session 768: every PCM packet arrived and Volcengine's raw
        // final contained the complete body, but stable attribution never moved
        // past the wake row. With no enrolled voiceprint, the wake-derived
        // profile is advisory; only repeated strong local NonTarget evidence may
        // veto this exact wake-only provider-schema recovery.
        return wake_end_ms.saturating_sub(wake_start_ms) <= MAX_ADAPTIVE_WAKE_ONLY_UTTERANCE_MS
            && state.local_non_target_speech_end_ms.is_none();
    }
    if !utterance_is_bounded_wake_phrase(stable_utterances[0], &normalized_phrase) {
        return false;
    }
    state
        .stable_attributed_speech_end_ms
        .is_some_and(|stable_end_ms| stable_end_ms > wake_end_ms.saturating_add(250))
}

/// Recover a provider-final tail when the debounced local identity never left
/// the owner but cloud diarization split that same speaker into another cluster.
/// This applies to both enrolled and wake-derived profiles and is intentionally
/// narrower than accepting all provider text: all stable utterances must be
/// sequential (no simultaneous speech), every foreign-cluster utterance must
/// have local evidence, and every overlapping local sample must still hold the
/// wake identity without a single NonTarget classification. This also covers
/// provider A/B/A cluster drift.
fn sequential_speaker_split_gap_is_owner_safe(
    state: &SyncState,
    provider_result: &Value,
    target_text: &str,
) -> bool {
    sequential_speaker_split_gap_is_owner_safe_internal(state, provider_result, target_text, false)
}

/// Final two-pass frames sometimes seal only the already-attributed prefix in
/// `utterances` while `result.text` still contains the recognized suffix. The
/// ordinary sequential-split gate intentionally rejects that schema during
/// live preview because the untimed suffix could still be room speech. At the
/// protocol-final boundary an enrolled, wake-verified owner may recover it
/// only when multiple local windows cover the omitted tail and every one still
/// holds the owner identity without a NonTarget/absence veto.
fn final_unsegmented_provider_tail_is_owner_safe(
    state: &SyncState,
    provider_result: &Value,
    target_text: &str,
) -> bool {
    sequential_speaker_split_gap_is_owner_safe_internal(state, provider_result, target_text, true)
}

fn sequential_speaker_split_gap_is_owner_safe_internal(
    state: &SyncState,
    provider_result: &Value,
    target_text: &str,
    allow_final_unsegmented_tail: bool,
) -> bool {
    if !state.local_speaker_tracking_enabled
        || !state.local_speaker_stable_target
        || state.owner_isolation_frozen
        || state.local_consecutive_non_target != 0
        || target_text.trim().is_empty()
    {
        return false;
    }
    let Some(target_speaker_id) = state.target_speaker_id.as_deref() else {
        return false;
    };
    let provider_text = provider_result
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if spoken_content_len(provider_text) <= spoken_content_len(target_text) {
        return false;
    }
    let normalize = |text: &str| {
        text.chars()
            .filter(|ch| ch.is_alphanumeric())
            .collect::<String>()
    };
    let normalized_provider = normalize(provider_text);
    let normalized_target = normalize(target_text);
    if normalized_target.is_empty() {
        return false;
    }

    let mut stable_utterances = provider_result
        .get("utterances")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|utterance| utterance_is_stable(utterance))
        .collect::<Vec<_>>();
    if stable_utterances.len() < 2 {
        return false;
    }
    stable_utterances.sort_by_key(|utterance| utterance_start_ms(utterance));
    let verified_owner_handoff = state.local_wake_owner_verified
        && !state.local_speaker_profile_adaptive
        && state.local_target_confirmed;
    if stable_utterances.windows(2).any(|pair| {
        let previous_end_ms = utterance_end_ms(pair[0]);
        let next_start_ms = utterance_start_ms(pair[1]);
        previous_end_ms
            .zip(next_start_ms)
            .map_or(true, |(end_ms, start_ms)| {
                start_ms < end_ms
                    && (!verified_owner_handoff
                        || end_ms.saturating_sub(start_ms)
                            > MAX_VERIFIED_OWNER_CLUSTER_HANDOFF_OVERLAP_MS)
            })
    }) {
        return false;
    }
    let attributed_text = stable_utterances
        .iter()
        .filter_map(|utterance| utterance.get("text").and_then(Value::as_str))
        .collect::<String>();
    let normalized_attributed = normalize(&attributed_text);
    let has_unsegmented_provider_tail = normalized_attributed != normalized_provider;
    if has_unsegmented_provider_tail
        && (!allow_final_unsegmented_tail
            || normalized_attributed.is_empty()
            || !normalized_provider.starts_with(&normalized_attributed))
    {
        return false;
    }
    let target_utterances = stable_utterances
        .iter()
        .copied()
        .filter(|utterance| utterance_speaker_id(utterance).as_deref() == Some(target_speaker_id))
        .collect::<Vec<_>>();
    let split_utterances = stable_utterances
        .iter()
        .copied()
        .filter(|utterance| {
            utterance_speaker_id(utterance)
                .as_deref()
                .is_some_and(|speaker_id| speaker_id != target_speaker_id)
        })
        .collect::<Vec<_>>();
    if target_utterances.is_empty() || split_utterances.is_empty() {
        return false;
    }
    let attributed_target_text = target_utterances
        .iter()
        .filter_map(|utterance| utterance.get("text").and_then(Value::as_str))
        .collect::<String>();
    if normalize(&attributed_target_text) != normalized_target {
        return false;
    }
    let split_utterances_are_owner_safe = split_utterances.iter().all(|utterance| {
        let (Some(start_ms), Some(end_ms)) =
            (utterance_start_ms(utterance), utterance_end_ms(utterance))
        else {
            return false;
        };
        let overlapping = state
            .local_speaker_evidence
            .iter()
            .filter(|sample| {
                let sample_center_ms = sample
                    .audio_end_ms
                    .saturating_sub(LOCAL_SPEAKER_WINDOW_MS / 2);
                sample_center_ms >= start_ms && sample_center_ms <= end_ms
            })
            .collect::<Vec<_>>();
        !overlapping.is_empty()
            && !local_non_target_vetoes_split_utterance(state, start_ms, end_ms)
            && overlapping.iter().all(|sample| {
                sample.stable_target
                    && !matches!(
                        sample.classification,
                        crate::speaker_verification::SessionSpeakerClassification::NonTarget { .. }
                    )
            })
    });
    if !split_utterances_are_owner_safe {
        return false;
    }
    if !has_unsegmented_provider_tail {
        return true;
    }

    // Untimed provider text is never admitted from an adaptive wake-derived
    // profile. Require the persisted owner voiceprint plus a positive body
    // confirmation and at least two debounced owner windows physically after
    // the last cloud-attributed row. This is the exact shape observed in the
    // 2026-08-22 owner acceptance: provider text reached 28 chars, final
    // utterances sealed at 18, and two later local owner windows covered the
    // missing suffix while BLE delivered the complete 7.36 s recording.
    if !state.local_wake_owner_verified
        || state.local_speaker_profile_adaptive
        || !state.local_target_confirmed
        || state.local_owner_absence_run_confirmed
    {
        return false;
    }
    let Some(tail_start_ms) = stable_utterances
        .iter()
        .filter_map(|utterance| utterance_end_ms(utterance))
        .max()
    else {
        return false;
    };
    let Some(tail_speech_end_ms) = state.local_speech_end_ms else {
        return false;
    };
    if tail_speech_end_ms <= tail_start_ms
        || local_non_target_vetoes_split_utterance(state, tail_start_ms, tail_speech_end_ms)
    {
        return false;
    }
    let tail_owner_windows = state
        .local_speaker_evidence
        .iter()
        .filter(|sample| {
            let sample_center_ms = sample
                .audio_end_ms
                .saturating_sub(LOCAL_SPEAKER_WINDOW_MS / 2);
            sample_center_ms > tail_start_ms && sample_center_ms <= tail_speech_end_ms
        })
        .collect::<Vec<_>>();
    tail_owner_windows.len() >= 2
        && tail_owner_windows.iter().all(|sample| {
            sample.stable_target
                && !matches!(
                    sample.classification,
                    crate::speaker_verification::SessionSpeakerClassification::NonTarget { .. }
                )
        })
}

fn local_non_target_vetoes_split_utterance(
    state: &SyncState,
    utterance_start_ms: u64,
    utterance_end_ms: u64,
) -> bool {
    let sample_center =
        |audio_end_ms: u64| audio_end_ms.saturating_sub(LOCAL_SPEAKER_WINDOW_MS / 2);
    let inside = |center_ms: u64| center_ms >= utterance_start_ms && center_ms <= utterance_end_ms;

    // The raw newest classification is useful only when its physical window
    // overlaps this provider utterance. Session 1195 ended with low-level room
    // noise classified NonTarget several seconds after the owner's final word;
    // the old global check discarded the already-complete owner tail.
    let newest_raw_non_target_overlaps = matches!(
        state.local_speaker_classification.as_ref(),
        Some(crate::speaker_verification::SessionSpeakerClassification::NonTarget { .. })
    ) && state
        .local_speaker_evidence
        .last()
        .is_some_and(|sample| inside(sample_center(sample.audio_end_ms)));
    if newest_raw_non_target_overlaps {
        return true;
    }

    let Some(non_target_audio_end_ms) = state.local_non_target_speech_end_ms else {
        return false;
    };
    let non_target_center_ms = sample_center(non_target_audio_end_ms);
    if !inside(non_target_center_ms) {
        return false;
    }

    // Endpoint NonTarget is intentionally sensitive and can fire twice on the
    // enrolled owner's cross-phrase acoustics. Treat it as a transcript veto
    // only while it remains unrecovered inside the same cloud utterance. A
    // later owner-compatible stable window (> absence threshold) proves that
    // the debounced wake identity continued; it does not weaken isolation for
    // a sustained real second speaker.
    !state.local_speaker_evidence.iter().any(|sample| {
        let center_ms = sample_center(sample.audio_end_ms);
        center_ms > non_target_center_ms
            && inside(center_ms)
            && sample.stable_target
            && sample.classification.score() > LOCAL_OWNER_ABSENCE_MAX_SCORE
    })
}

fn result_marks_two_pass_empty(result: &Value) -> bool {
    result
        .get("utterances")
        .and_then(Value::as_array)
        .is_some_and(|utterances| {
            !utterances.is_empty()
                && utterances.iter().all(|utterance| {
                    let text_empty = utterance
                        .get("text")
                        .and_then(Value::as_str)
                        .map_or(true, |text| text.trim().is_empty());
                    let flagged_empty = utterance
                        .get("additions")
                        .and_then(|additions| additions.get("two_pass_empty"))
                        .is_some_and(|flag| match flag {
                            Value::Bool(true) => true,
                            Value::String(value) => {
                                value.eq_ignore_ascii_case("true") || value == "1"
                            }
                            Value::Number(value) => value.as_u64() == Some(1),
                            _ => false,
                        });
                    text_empty && flagged_empty
                })
        })
}

/// Session speech ledger for empty/weaker protocol finals.
///
/// Prefer Target-gated best text. When multi-speaker isolation has observed
/// NonTarget (or is frozen), never promote a longer optimistic preview that may
/// have absorbed room speech before the dual-gate closed.
fn session_committed_transcript(state: &SyncState) -> Option<(String, Vec<TranscriptSegment>)> {
    if state.local_speaker_tracking_enabled
        && (state.owner_isolation_frozen || state.local_non_target_speech_end_ms.is_some())
    {
        let ceiling = if state.owner_isolation_frozen
            && !state.owner_isolation_ceiling_text.trim().is_empty()
        {
            (
                state.owner_isolation_ceiling_text.as_str(),
                &state.owner_isolation_ceiling_segments,
            )
        } else {
            (
                state.best_transcript_text.as_str(),
                &state.best_transcript_segments,
            )
        };
        let best = (
            state.best_transcript_text.as_str(),
            &state.best_transcript_segments,
        );
        let last = (
            state.last_partial_text.as_str(),
            &state.best_transcript_segments,
        );
        return [ceiling, best, last]
            .into_iter()
            .filter(|(text, _)| !text.trim().is_empty())
            // Prefer *shorter* owner-safe text over a longer polluted ledger:
            // among non-empty candidates pick the longest that does not exceed
            // the isolation ceiling when frozen.
            .max_by_key(|(text, _)| {
                let len = spoken_content_len(text);
                if state.owner_isolation_frozen
                    && spoken_content_len(text)
                        > spoken_content_len(&state.owner_isolation_ceiling_text)
                {
                    0
                } else {
                    len
                }
            })
            .map(|(text, segments)| (text.to_string(), segments.clone()));
    }
    let candidates = [
        (
            state.best_transcript_text.as_str(),
            &state.best_transcript_segments,
        ),
        (
            state.last_partial_text.as_str(),
            &state.best_transcript_segments,
        ),
        (
            state.optimistic_preview_text.as_str(),
            &state.optimistic_preview_segments,
        ),
    ];
    candidates
        .into_iter()
        .filter(|(text, _)| !text.trim().is_empty())
        .max_by_key(|(text, _)| spoken_content_len(text))
        .map(|(text, segments)| (text.to_string(), segments.clone()))
}

fn commit_session_transcript_if_stronger(
    state: &mut SyncState,
    text: &str,
    segments: Vec<TranscriptSegment>,
) {
    if text.trim().is_empty() {
        return;
    }
    if spoken_content_len(text) < spoken_content_len(&state.best_transcript_text) {
        return;
    }
    let (text, segments) = clamp_to_owner_isolation_ceiling(state, text.to_string(), segments);
    if spoken_content_len(&text) < spoken_content_len(&state.best_transcript_text) {
        return;
    }
    state.best_transcript_text = text.clone();
    state.best_transcript_segments = segments;
    state.last_partial_text = text;
}

fn confirmed_owner_final_preview_fallback(
    state: &SyncState,
    retained_text: &str,
) -> Option<(String, Vec<TranscriptSegment>)> {
    // Protocol contract: an empty / weaker final never regresses below the
    // session speech ledger. Content only enters that ledger from Target-gated
    // optimistic stream or already-filtered best text, so later NonTarget tails
    // and Volcengine `two_pass_empty` seals cannot invent foreign speech here.
    let (committed_text, committed_segments) = session_committed_transcript(state)?;
    (spoken_content_len(&committed_text) > spoken_content_len(retained_text))
        .then_some((committed_text, committed_segments))
}

fn final_partial_coverage_gap_from_state(state: &SyncState) -> Option<(u64, u64)> {
    if !state.finishing || state.best_transcript_text.trim().is_empty() {
        return None;
    }
    let transcript_end_ms = state
        .best_transcript_segments
        .iter()
        .filter_map(|segment| segment.end_ms)
        .max()
        .and_then(|end_ms| u64::try_from(end_ms).ok())?;
    let sent_audio_ms = (state.bytes_sent as f64 / BYTES_PER_MS) as u64;
    if state
        .last_server_audio_duration_ms
        .is_some_and(|duration_ms| {
            duration_ms.saturating_add(FINAL_PARTIAL_COVERAGE_SLACK_MS) >= sent_audio_ms
        })
    {
        return None;
    }
    (transcript_end_ms.saturating_add(FINAL_PARTIAL_COVERAGE_SLACK_MS) < sent_audio_ms)
        .then_some((sent_audio_ms, transcript_end_ms))
}

fn server_audio_duration_ms(json: &Value) -> Option<u64> {
    json.get("audio_info")
        .and_then(|audio_info| audio_info.get("duration"))
        .and_then(Value::as_u64)
}

struct SpeakerFilteredResult {
    result: Value,
    optimistic_result: Value,
    speaker_info_present: bool,
    stable_non_target_utterance_present: bool,
    target_speech_end_ms: Option<u64>,
    wake_target_speech_end_ms: Option<u64>,
    stable_attributed_speech_end_ms: Option<u64>,
    pending_unattributed_text: String,
}

fn spoken_content_len(text: &str) -> usize {
    text.chars().filter(|ch| ch.is_alphanumeric()).count()
}

fn utterance_speaker_id(utterance: &Value) -> Option<String> {
    let value = utterance
        .get("additions")
        .and_then(|additions| {
            additions
                .get("speaker")
                .or_else(|| additions.get("speaker_id"))
        })
        .or_else(|| utterance.get("speaker"))?;
    if let Some(value) = value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Some(value.to_string());
    }
    if let Some(value) = value.as_i64() {
        return Some(value.to_string());
    }
    value.as_u64().map(|value| value.to_string())
}

fn utterance_end_ms(utterance: &Value) -> Option<u64> {
    let word_end = utterance
        .get("words")
        .and_then(Value::as_array)
        .and_then(|words| words.iter().rev().find_map(utterance_time_value))
        .and_then(time_value_ms);
    let utterance_end = utterance_time_value(utterance).and_then(time_value_ms);
    word_end.into_iter().chain(utterance_end).max()
}

fn utterance_start_ms(utterance: &Value) -> Option<u64> {
    let utterance_start = ["start_time", "startTime", "start_ms"]
        .iter()
        .find_map(|key| utterance.get(*key))
        .and_then(time_value_ms);
    let word_start = utterance
        .get("words")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|word| {
            ["start_time", "startTime", "start_ms"]
                .iter()
                .find_map(|key| word.get(*key))
                .and_then(time_value_ms)
        })
        .min();
    utterance_start.into_iter().chain(word_start).min()
}

fn time_value_ms(value: &Value) -> Option<u64> {
    if let Some(value) = value.as_u64() {
        return Some(value);
    }
    if let Some(value) = value.as_i64() {
        return u64::try_from(value).ok();
    }
    value.as_str()?.trim().parse::<u64>().ok()
}

fn utterance_is_stable(utterance: &Value) -> bool {
    utterance
        .get("definite")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn utterance_time_value(value: &Value) -> Option<&Value> {
    ["end_time", "endTime", "end_ms"]
        .iter()
        .find_map(|key| value.get(*key))
}

fn filter_result_to_target_speaker(
    result: &Value,
    target_speaker_id: &mut Option<String>,
) -> SpeakerFilteredResult {
    filter_result_to_target_speaker_with_local_evidence(result, target_speaker_id, false, &[], None)
}

fn locally_bound_target_speaker(
    utterances: &[Value],
    evidence: &[LocalSpeakerEvidence],
) -> Option<String> {
    let mut votes = std::collections::BTreeMap::<String, (u32, u32)>::new();
    for utterance in utterances
        .iter()
        .filter(|utterance| utterance_is_stable(utterance))
    {
        let Some(speaker_id) = utterance_speaker_id(utterance) else {
            continue;
        };
        let Some(end_ms) = utterance_end_ms(utterance) else {
            continue;
        };
        let start_ms = utterance_start_ms(utterance).unwrap_or(end_ms);
        let entry = votes.entry(speaker_id).or_default();
        for sample in evidence {
            let sample_start_ms = sample.audio_end_ms.saturating_sub(LOCAL_SPEAKER_WINDOW_MS);
            if sample.audio_end_ms < start_ms || sample_start_ms > end_ms {
                continue;
            }
            match sample.classification {
                crate::speaker_verification::SessionSpeakerClassification::Target { .. }
                    if sample.stable_target =>
                {
                    entry.0 += 1
                }
                crate::speaker_verification::SessionSpeakerClassification::NonTarget { .. }
                    if !sample.stable_target =>
                {
                    entry.1 += 1
                }
                _ => {}
            }
        }
    }
    votes
        .into_iter()
        .filter(|(_, (target_votes, _))| *target_votes > 0)
        .max_by(
            |(left_id, (left_target, left_other)), (right_id, (right_target, right_other))| {
                let left_score = i64::from(*left_target) - i64::from(*left_other);
                let right_score = i64::from(*right_target) - i64::from(*right_other);
                left_score
                    .cmp(&right_score)
                    .then_with(|| left_target.cmp(right_target))
                    .then_with(|| right_id.cmp(left_id))
            },
        )
        .map(|(speaker_id, _)| speaker_id)
}

fn wake_phrase_bound_target_speaker(utterances: &[Value], wake_phrase: &str) -> Option<String> {
    let normalized_phrase = wake_phrase
        .chars()
        .filter(|ch| ch.is_alphanumeric())
        .collect::<String>();
    if normalized_phrase.is_empty() {
        return None;
    }
    utterances.iter().find_map(|utterance| {
        if !utterance_is_stable(utterance) {
            return None;
        }
        utterance_contains_normalized_phrase(utterance, &normalized_phrase)
            .then(|| utterance_speaker_id(utterance))
            .flatten()
    })
}

fn utterance_contains_normalized_phrase(utterance: &Value, normalized_phrase: &str) -> bool {
    !normalized_phrase.is_empty()
        && utterance
            .get("text")
            .and_then(Value::as_str)
            .map(|text| {
                text.chars()
                    .filter(|ch| ch.is_alphanumeric())
                    .collect::<String>()
                    .contains(normalized_phrase)
            })
            .unwrap_or(false)
}

fn utterance_is_bounded_wake_phrase(utterance: &Value, normalized_phrase: &str) -> bool {
    if !utterance_contains_normalized_phrase(utterance, normalized_phrase) {
        return false;
    }
    let Some(start_ms) = utterance_start_ms(utterance) else {
        return false;
    };
    let Some(end_ms) = utterance_end_ms(utterance) else {
        return false;
    };
    end_ms.saturating_sub(start_ms) <= MAX_WAKE_PHRASE_UTTERANCE_MS
}

fn local_evidence_allows_utterance(
    utterance: &Value,
    evidence: &[LocalSpeakerEvidence],
    wake_speaker_phrase: Option<&str>,
) -> bool {
    let normalized_phrase = wake_speaker_phrase
        .unwrap_or_default()
        .chars()
        .filter(|ch| ch.is_alphanumeric())
        .collect::<String>();
    if utterance_is_bounded_wake_phrase(utterance, &normalized_phrase) {
        return true;
    }
    let Some(end_ms) = utterance_end_ms(utterance) else {
        return false;
    };
    let start_ms = utterance_start_ms(utterance).unwrap_or(end_ms);
    let mut target_votes = 0u32;
    let mut non_target_votes = 0u32;
    for sample in evidence {
        let sample_center_ms = sample
            .audio_end_ms
            .saturating_sub(LOCAL_SPEAKER_WINDOW_MS / 2);
        if sample_center_ms < start_ms || sample_center_ms > end_ms {
            continue;
        }
        match sample.classification {
            // Count Target only while debounced identity is still the owner.
            crate::speaker_verification::SessionSpeakerClassification::Target { .. }
                if sample.stable_target =>
            {
                target_votes += 1
            }
            // Count *every* NonTarget sample that overlaps the utterance — even
            // while hysteresis still holds stable_target. Industry TS-ASR /
            // dual-gate practice: non-owner evidence vetoes the window.
            crate::speaker_verification::SessionSpeakerClassification::NonTarget { .. } => {
                non_target_votes += 1
            }
            _ => {}
        }
    }
    // Strict isolation: owner needs a pure majority. Tie or NonTarget-only → reject.
    target_votes > 0 && target_votes > non_target_votes
}

/// Cloud diarization remains the baseline owner signal while an overlapping
/// local verifier window still has the debounced owner identity. Individual
/// NonTarget samples can be noisy (the verifier deliberately keeps
/// `stable_target` true during them), so they must not erase an utterance
/// already attributed to the wake speaker by the provider. With no overlapping
/// verifier window, existing preview/final fallback remains authoritative.
fn local_evidence_supports_cloud_target(
    utterance: &Value,
    evidence: &[LocalSpeakerEvidence],
    wake_speaker_phrase: Option<&str>,
) -> bool {
    let Some(end_ms) = utterance_end_ms(utterance) else {
        return false;
    };
    let start_ms = utterance_start_ms(utterance).unwrap_or(end_ms);
    let mut has_stable_owner_overlap = false;
    for sample in evidence {
        let sample_center_ms = sample
            .audio_end_ms
            .saturating_sub(LOCAL_SPEAKER_WINDOW_MS / 2);
        if sample_center_ms < start_ms || sample_center_ms > end_ms {
            continue;
        }
        if !sample.stable_target {
            return false;
        }
        has_stable_owner_overlap = true;
    }
    has_stable_owner_overlap
        && !local_evidence_confirms_owner_absence(utterance, evidence, wake_speaker_phrase)
}

fn local_evidence_confirms_owner_absence(
    utterance: &Value,
    evidence: &[LocalSpeakerEvidence],
    wake_speaker_phrase: Option<&str>,
) -> bool {
    local_evidence_confirms_owner_absence_with_verified_wake(
        utterance,
        evidence,
        wake_speaker_phrase,
        false,
        false,
        None,
    )
}

fn local_evidence_confirms_owner_absence_with_verified_wake(
    utterance: &Value,
    evidence: &[LocalSpeakerEvidence],
    wake_speaker_phrase: Option<&str>,
    wake_owner_verified: bool,
    local_speaker_profile_adaptive: bool,
    confirmed_non_target_speech_end_ms: Option<u64>,
) -> bool {
    let Some(end_ms) = utterance_end_ms(utterance) else {
        return false;
    };
    let start_ms = utterance_start_ms(utterance).unwrap_or(end_ms);
    let normalized_wake_phrase = wake_speaker_phrase
        .unwrap_or_default()
        .chars()
        .filter(|ch| ch.is_alphanumeric())
        .collect::<String>();
    let wake_anchored_utterance =
        utterance_contains_normalized_phrase(utterance, &normalized_wake_phrase);
    // Low-score Uncertain windows are only meaningful owner-absence evidence
    // after this session's body tracker has demonstrated that it can positively
    // recognize the owner at least once. Otherwise an enrollment/acoustic
    // mismatch can leave every post-wake window Uncertain while the debounced
    // identity still belongs to the verified wake speaker. Treating that
    // never-calibrated run as a speaker switch destructively removed complete
    // owner dictation in installed sessions 1 and 27 (the provider retained 56
    // and 31 chars respectively, while filtering kept only the wake/tail).
    //
    // A real debounced identity departure remains fail-closed below regardless
    // of whether a Target window was observed, and explicit Target-confirmed
    // sessions retain the strict same-cloud-cluster isolation rule.
    let session_has_stable_target = evidence.iter().any(|sample| {
        sample.stable_target
            && matches!(
                sample.classification,
                crate::speaker_verification::SessionSpeakerClassification::Target { .. }
            )
    });
    // The wake gate is already a positive persisted-owner observation, but it
    // must not by itself delete later text: cross-phrase scores can be low for
    // the real owner.  Promote it to owner-absence authority only when the
    // enrolled (non-adaptive) body verifier also produced two consecutive
    // <=0.20 NonTarget windows inside this exact provider utterance.  Three
    // overlapping <=0.30 samples below then form the independent duration
    // check. This recovers the installed failure where another person reused
    // cloud speaker 0 without requiring a lucky positive body-window first.
    let confirmed_non_target_overlaps_utterance = wake_owner_verified
        && !local_speaker_profile_adaptive
        && confirmed_non_target_speech_end_ms.is_some_and(|audio_end_ms| {
            let sample_center_ms = audio_end_ms.saturating_sub(LOCAL_SPEAKER_WINDOW_MS / 2);
            sample_center_ms >= start_ms && sample_center_ms <= end_ms
        });
    let mut overlap_count = 0u32;
    let mut target_votes = 0u32;
    let mut owner_absence_votes = 0u32;
    for sample in evidence {
        let sample_center_ms = sample
            .audio_end_ms
            .saturating_sub(LOCAL_SPEAKER_WINDOW_MS / 2);
        if sample_center_ms < start_ms || sample_center_ms > end_ms {
            continue;
        }
        overlap_count += 1;
        if !sample.stable_target {
            return true;
        }
        if matches!(
            sample.classification,
            crate::speaker_verification::SessionSpeakerClassification::Target { .. }
        ) {
            target_votes += 1;
        }
        if sample.classification.score() <= LOCAL_OWNER_ABSENCE_MAX_SCORE {
            owner_absence_votes += 1;
        }
    }
    // Volcengine sometimes seals one long two-pass utterance containing both
    // the confirmed wake and every clause after a natural pause. In that shape,
    // a run of low-score `Uncertain` windows is not a separate speaker boundary:
    // the tracker still owns the identity (`stable_target=true`). Rejecting the
    // whole wake-anchored utterance deleted the owner's post-pause tail in
    // installed session 52. A real debounced identity departure still returns
    // above, while separate later utterances retain the strict owner-absence
    // rule used for same-cloud-cluster multi-speaker isolation.
    if wake_anchored_utterance {
        return false;
    }
    (session_has_stable_target || confirmed_non_target_overlaps_utterance)
        && overlap_count > 0
        && target_votes == 0
        && owner_absence_votes >= LOCAL_OWNER_ABSENCE_CONFIRMATIONS
}

/// Clamp a growing transcript to the frozen owner ceiling when multi-speaker
/// isolation is active (stable identity left the wake speaker).
fn clamp_to_owner_isolation_ceiling(
    state: &SyncState,
    text: String,
    segments: Vec<TranscriptSegment>,
) -> (String, Vec<TranscriptSegment>) {
    if !state.local_speaker_tracking_enabled || !state.owner_isolation_frozen {
        return (text, segments);
    }
    if spoken_content_len(&text) <= spoken_content_len(&state.owner_isolation_ceiling_text) {
        return (text, segments);
    }
    (
        state.owner_isolation_ceiling_text.clone(),
        state.owner_isolation_ceiling_segments.clone(),
    )
}

fn freeze_owner_isolation_ledger(state: &mut SyncState) {
    if !state.local_speaker_tracking_enabled || state.owner_isolation_frozen {
        return;
    }
    // Prefer the longer of best vs already Target-gated optimistic at freeze time.
    // Both should already exclude room speech after optimistic gates; ceiling is
    // a hard cap against later polluted finals.
    let (text, segments) = if spoken_content_len(&state.best_transcript_text)
        >= spoken_content_len(&state.optimistic_preview_text)
    {
        (
            state.best_transcript_text.clone(),
            state.best_transcript_segments.clone(),
        )
    } else {
        (
            state.optimistic_preview_text.clone(),
            state.optimistic_preview_segments.clone(),
        )
    };
    state.owner_isolation_ceiling_text = text;
    state.owner_isolation_ceiling_segments = segments;
    state.owner_isolation_frozen = true;
    log::info!(
        "[asr] owner isolation freeze ceiling_chars={} best_chars={} optimistic_chars={}",
        state.owner_isolation_ceiling_text.chars().count(),
        state.best_transcript_text.chars().count(),
        state.optimistic_preview_text.chars().count()
    );
}

/// Freeze at the provider's already-filtered owner utterances when cloud
/// diarization reused the owner's speaker id for a locally rejected later
/// utterance. This is stronger than a raw voiceprint negative: a stable
/// provider boundary and a sustained local owner-absence run must agree first.
fn freeze_owner_isolation_at_filtered_result(state: &mut SyncState, filtered_result: &Value) {
    if !state.local_speaker_tracking_enabled {
        return;
    }
    let candidate = transcript_candidate_from_result(filtered_result);
    if candidate.text.trim().is_empty() {
        return;
    }
    state.owner_isolation_ceiling_text = candidate.text.clone();
    state.owner_isolation_ceiling_segments = candidate.timed_segments.clone();
    state.owner_isolation_frozen = true;

    // A provisional preview may already contain the other person's words while
    // diarization was pending. Replace every committed/display ledger with the
    // filtered owner text so protocol-final fallback cannot restore that tail.
    state.best_transcript_text = candidate.text.clone();
    state.best_transcript_segments = candidate.timed_segments.clone();
    state.best_untimed_window.clear();
    state.last_partial_text = candidate.text.clone();
    state.optimistic_preview_text = candidate.text;
    state.optimistic_preview_segments = candidate.timed_segments;
    state.optimistic_untimed_window.clear();
    log::info!(
        "[asr] owner isolation froze at filtered stable utterances ceiling_chars={} reason=stable_same_cluster_owner_absence",
        state.owner_isolation_ceiling_text.chars().count()
    );
}

fn unfreeze_owner_isolation_ledger(state: &mut SyncState) {
    if !state.owner_isolation_frozen {
        return;
    }
    state.owner_isolation_frozen = false;
    state.owner_isolation_ceiling_text.clear();
    state.owner_isolation_ceiling_segments.clear();
    log::info!("[asr] owner isolation unfrozen after Target restabilized");
}

fn utterance_belongs_to_target(
    utterance: &Value,
    target_speaker_id: &str,
    local_speaker_tracking_enabled: bool,
    local_speaker_evidence: &[LocalSpeakerEvidence],
    wake_speaker_phrase: Option<&str>,
    wake_speaker_end_ms: Option<u64>,
    allow_immediate_wake_continuation: bool,
) -> bool {
    if !utterance_is_stable(utterance) {
        return false;
    }

    let cloud_id_matches = utterance_speaker_id(utterance).as_deref() == Some(target_speaker_id);
    if !local_speaker_tracking_enabled {
        return cloud_id_matches;
    }

    if let Some(wake_phrase) = wake_speaker_phrase {
        let normalized_phrase = wake_phrase
            .chars()
            .filter(|ch| ch.is_alphanumeric())
            .collect::<String>();
        if utterance_is_bounded_wake_phrase(utterance, &normalized_phrase) {
            return cloud_id_matches;
        }
    }

    // In an automatic session the local wake-speaker timeline owns identity.
    // Cloud speaker IDs are diarization clusters and may split one real speaker.
    // Keep the baseline cloud-owner result through Uncertain and isolated noisy
    // NonTarget windows; only a locally debounced identity switch can veto it.
    if cloud_id_matches {
        return local_evidence_supports_cloud_target(
            utterance,
            local_speaker_evidence,
            wake_speaker_phrase,
        );
    }

    // Volcengine can seal the physical wake as cluster 0, then immediately
    // roll the same uninterrupted speaker's body into cluster 1. Session
    // b6c7bd31 exposed the rolling-response form: later packets contained only
    // the cluster-1 body, while every local identity sample still retained the
    // verified wake owner (albeit Uncertain across phrases). Accept only the
    // first, tightly contiguous post-wake boundary. A later speaker change, a
    // gap, or any debounced local departure stays excluded.
    let immediate_wake_continuation = allow_immediate_wake_continuation
        && wake_speaker_end_ms.is_some_and(|wake_end_ms| {
            utterance_start_ms(utterance).is_some_and(|start_ms| {
                start_ms >= wake_end_ms.saturating_sub(200)
                    && start_ms <= wake_end_ms.saturating_add(MAX_WAKE_BODY_CLUSTER_SPLIT_GAP_MS)
            }) && local_evidence_supports_cloud_target(
                utterance,
                local_speaker_evidence,
                wake_speaker_phrase,
            )
        });
    if immediate_wake_continuation {
        return true;
    }

    // Later provider speaker-ID splits still require strict positive local
    // owner evidence so ordinary sequential room speech remains excluded.
    let local_target =
        local_evidence_allows_utterance(utterance, local_speaker_evidence, wake_speaker_phrase);
    local_target
        && wake_speaker_end_ms.is_some_and(|wake_end_ms| {
            utterance_start_ms(utterance).is_some_and(|start_ms| start_ms >= wake_end_ms)
        })
}

fn utterance_belongs_to_verified_target(
    utterance: &Value,
    target_speaker_id: &str,
    local_speaker_tracking_enabled: bool,
    local_speaker_evidence: &[LocalSpeakerEvidence],
    wake_speaker_phrase: Option<&str>,
    wake_speaker_end_ms: Option<u64>,
    allow_immediate_wake_continuation: bool,
    wake_owner_verified: bool,
    local_speaker_profile_adaptive: bool,
    confirmed_non_target_speech_end_ms: Option<u64>,
) -> bool {
    let baseline_belongs = utterance_belongs_to_target(
        utterance,
        target_speaker_id,
        local_speaker_tracking_enabled,
        local_speaker_evidence,
        wake_speaker_phrase,
        wake_speaker_end_ms,
        allow_immediate_wake_continuation,
    );
    let verified_wake_anchor_belongs = !baseline_belongs
        && wake_owner_verified
        && utterance_speaker_id(utterance).as_deref() == Some(target_speaker_id)
        && wake_speaker_phrase.is_some_and(|phrase| {
            let normalized_phrase = phrase
                .chars()
                .filter(|ch| ch.is_alphanumeric())
                .collect::<String>();
            utterance_contains_normalized_phrase(utterance, &normalized_phrase)
        });
    if !baseline_belongs && !verified_wake_anchor_belongs {
        return false;
    }
    // Cloud can reuse the owner's speaker id for the next real person. The
    // verified-wake path supplies the missing identity authority, while the
    // sustained per-utterance low-score checks keep ordinary owner dips safe.
    !(utterance_speaker_id(utterance).as_deref() == Some(target_speaker_id)
        && local_evidence_confirms_owner_absence_with_verified_wake(
            utterance,
            local_speaker_evidence,
            wake_speaker_phrase,
            wake_owner_verified,
            local_speaker_profile_adaptive,
            confirmed_non_target_speech_end_ms,
        ))
}

fn filter_result_to_target_speaker_with_local_evidence(
    result: &Value,
    target_speaker_id: &mut Option<String>,
    local_speaker_tracking_enabled: bool,
    local_speaker_evidence: &[LocalSpeakerEvidence],
    wake_speaker_phrase: Option<&str>,
) -> SpeakerFilteredResult {
    filter_result_to_target_speaker_with_local_evidence_and_anchor(
        result,
        target_speaker_id,
        local_speaker_tracking_enabled,
        local_speaker_evidence,
        wake_speaker_phrase,
        None,
        false,
        false,
        None,
    )
}

fn filter_result_to_target_speaker_with_local_evidence_and_anchor(
    result: &Value,
    target_speaker_id: &mut Option<String>,
    local_speaker_tracking_enabled: bool,
    local_speaker_evidence: &[LocalSpeakerEvidence],
    wake_speaker_phrase: Option<&str>,
    prior_wake_speaker_end_ms: Option<u64>,
    wake_owner_verified: bool,
    local_speaker_profile_adaptive: bool,
    confirmed_non_target_speech_end_ms: Option<u64>,
) -> SpeakerFilteredResult {
    let mut filtered_result = result.clone();
    let utterances = result
        .get("utterances")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let speaker_info_present = utterances.iter().any(|utterance| {
        utterance_is_stable(utterance) && utterance_speaker_id(utterance).is_some()
    });

    if target_speaker_id.is_none() {
        *target_speaker_id = if local_speaker_tracking_enabled {
            wake_speaker_phrase
                .and_then(|phrase| wake_phrase_bound_target_speaker(&utterances, phrase))
                .or_else(|| locally_bound_target_speaker(&utterances, local_speaker_evidence))
        } else {
            utterances.iter().find_map(|utterance| {
                let text = utterance.get("text").and_then(Value::as_str)?.trim();
                (utterance_is_stable(utterance) && !text.is_empty())
                    .then(|| utterance_speaker_id(utterance))
                    .flatten()
            })
        };
    }
    let observed_wake_speaker_end_ms = target_speaker_id.as_deref().and_then(|target| {
        let normalized_phrase = wake_speaker_phrase?
            .chars()
            .filter(|ch| ch.is_alphanumeric())
            .collect::<String>();
        utterances
            .iter()
            .filter(|utterance| {
                utterance_is_stable(utterance)
                    && utterance_speaker_id(utterance).as_deref() == Some(target)
                    && utterance_contains_normalized_phrase(utterance, &normalized_phrase)
            })
            .filter_map(utterance_end_ms)
            .max()
    });
    let fallback_wake_speaker_end_ms = target_speaker_id.as_deref().and_then(|target| {
        if observed_wake_speaker_end_ms.is_some() || !local_speaker_tracking_enabled {
            return None;
        }
        // The cloud can recognize the physical wake as a near-homophone
        // (for example "此录音") and then split the same real speaker's body
        // into a new diarization cluster. This fallback is response-local and
        // is never persisted as the verified physical-wake boundary.
        utterances
            .iter()
            .filter(|utterance| {
                utterance_is_stable(utterance)
                    && utterance_speaker_id(utterance).as_deref() == Some(target)
            })
            .filter_map(utterance_end_ms)
            .min()
    });
    let wake_speaker_end_ms = observed_wake_speaker_end_ms
        .or(fallback_wake_speaker_end_ms)
        .or(prior_wake_speaker_end_ms);
    // Only the rolling-response shape needs the continuity exception: the
    // verified wake was present in an earlier packet and this packet contains
    // no stable row for that cluster. Full multi-utterance responses continue
    // through the stricter existing A/B/A recovery path.
    let allow_immediate_wake_continuation = prior_wake_speaker_end_ms.is_some()
        && target_speaker_id.as_deref().is_some_and(|target| {
            !utterances.iter().any(|utterance| {
                utterance_is_stable(utterance)
                    && utterance_speaker_id(utterance).as_deref() == Some(target)
            })
        });

    let selected = target_speaker_id
        .as_deref()
        .map(|target| {
            utterances
                .iter()
                .filter(|utterance| {
                    utterance_belongs_to_verified_target(
                        utterance,
                        target,
                        local_speaker_tracking_enabled,
                        local_speaker_evidence,
                        wake_speaker_phrase,
                        wake_speaker_end_ms,
                        allow_immediate_wake_continuation,
                        wake_owner_verified,
                        local_speaker_profile_adaptive,
                        confirmed_non_target_speech_end_ms,
                    )
                })
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let target_text = selected
        .iter()
        .filter_map(|utterance| utterance.get("text").and_then(Value::as_str))
        .collect::<String>();
    let attributed_text = result
        .get("utterances")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|utterance| utterance_is_stable(utterance))
        .filter_map(|utterance| utterance.get("text").and_then(Value::as_str))
        .collect::<String>();
    let unstable_text = result
        .get("utterances")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|utterance| !utterance_is_stable(utterance))
        .filter_map(|utterance| utterance.get("text").and_then(Value::as_str))
        .collect::<String>();
    let raw_text = result
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let raw_has_unattributed_tail =
        spoken_content_len(raw_text) > spoken_content_len(&attributed_text);
    let pending_unattributed_speech = !unstable_text.trim().is_empty() || raw_has_unattributed_tail;
    let stable_other_speaker_present = target_speaker_id.as_deref().is_some_and(|target| {
        utterances.iter().any(|utterance| {
            utterance_is_stable(utterance)
                && utterance_speaker_id(utterance).is_some()
                && !utterance_belongs_to_verified_target(
                    utterance,
                    target,
                    local_speaker_tracking_enabled,
                    local_speaker_evidence,
                    wake_speaker_phrase,
                    wake_speaker_end_ms,
                    allow_immediate_wake_continuation,
                    wake_owner_verified,
                    local_speaker_profile_adaptive,
                    confirmed_non_target_speech_end_ms,
                )
        })
    });
    // Hard exclusion is intentionally narrower than ordinary cloud
    // diarization filtering. It addresses the field failure where the cloud
    // reused the *same* speaker id for a later real person. Missing local
    // evidence and cloud A/B cluster drift are not hard exclusion evidence and
    // retain their existing final-recovery paths.
    let stable_same_cluster_owner_absence_present =
        target_speaker_id.as_deref().is_some_and(|target| {
            utterances.iter().any(|utterance| {
                utterance_is_stable(utterance)
                    && utterance_speaker_id(utterance).as_deref() == Some(target)
                    && local_evidence_confirms_owner_absence_with_verified_wake(
                        utterance,
                        local_speaker_evidence,
                        wake_speaker_phrase,
                        wake_owner_verified,
                        local_speaker_profile_adaptive,
                        confirmed_non_target_speech_end_ms,
                    )
            })
        });
    // Unstable stream tails are only "pending owner text" when local evidence
    // still votes Target for that window. Blindly including every indefinite
    // utterance let other people ride into the capsule/ledger while cloud
    // diarization lagged (same speaker id or stream-only segments).
    let optimistic_utterances = utterances
        .iter()
        .filter(|utterance| {
            if target_speaker_id.as_deref().is_some_and(|target| {
                utterance_belongs_to_verified_target(
                    utterance,
                    target,
                    local_speaker_tracking_enabled,
                    local_speaker_evidence,
                    wake_speaker_phrase,
                    wake_speaker_end_ms,
                    allow_immediate_wake_continuation,
                    wake_owner_verified,
                    local_speaker_profile_adaptive,
                    confirmed_non_target_speech_end_ms,
                )
            }) {
                return true;
            }
            if utterance_is_stable(utterance) {
                return false;
            }
            if !local_speaker_tracking_enabled {
                return true;
            }
            local_evidence_allows_utterance(utterance, local_speaker_evidence, wake_speaker_phrase)
        })
        .cloned()
        .collect::<Vec<_>>();
    let optimistic_utterance_text = optimistic_utterances
        .iter()
        .filter_map(|utterance| utterance.get("text").and_then(Value::as_str))
        .collect::<String>();
    // Local NonTarget samples (even before debounce flips stable_target) must
    // freeze optimistic growth at the Target-filtered text so room speech cannot
    // inflate the session ledger during the switch confirmation window.
    let local_latest_is_non_target = local_speaker_evidence.last().is_some_and(|sample| {
        matches!(
            sample.classification,
            crate::speaker_verification::SessionSpeakerClassification::NonTarget { .. }
        )
    });
    let optimistic_text = if !pending_unattributed_speech
        || (local_speaker_tracking_enabled && local_latest_is_non_target)
    {
        target_text.clone()
    } else if !optimistic_utterance_text.trim().is_empty()
        && spoken_content_len(&optimistic_utterance_text) > spoken_content_len(&target_text)
    {
        optimistic_utterance_text
    } else if !stable_other_speaker_present
        && raw_has_unattributed_tail
        && !local_speaker_tracking_enabled
    {
        // Without local tracking the raw tail is the only provisional channel.
        // With local tracking, raw often mixes room speakers under one stream —
        // never promote it over Target-filtered text.
        raw_text.to_string()
    } else if !stable_other_speaker_present
        && raw_has_unattributed_tail
        && !local_latest_is_non_target
        && local_speaker_evidence
            .last()
            .is_some_and(|sample| sample.stable_target)
    {
        if let Some(tail) = raw_text.strip_prefix(&attributed_text) {
            format!("{target_text}{tail}")
        } else {
            target_text.clone()
        }
    } else if let Some(tail) = raw_text.strip_prefix(&attributed_text) {
        if stable_other_speaker_present || local_latest_is_non_target {
            target_text.clone()
        } else {
            format!("{target_text}{tail}")
        }
    } else {
        target_text.clone()
    };
    let pending_unattributed_text = pending_unattributed_speech
        .then(|| optimistic_text.clone())
        .unwrap_or_default();
    let target_speech_end_ms = selected
        .iter()
        .filter(|utterance| utterance_is_stable(utterance))
        .filter_map(|utterance| utterance_end_ms(utterance))
        .max();
    let stable_attributed_speech_end_ms = utterances
        .iter()
        .filter(|utterance| {
            utterance_is_stable(utterance) && utterance_speaker_id(utterance).is_some()
        })
        .filter_map(utterance_end_ms)
        .max();

    filtered_result["utterances"] = Value::Array(selected);
    filtered_result["text"] = Value::String(target_text);
    let mut optimistic_result = result.clone();
    optimistic_result["utterances"] = Value::Array(optimistic_utterances);
    optimistic_result["text"] = Value::String(optimistic_text);
    SpeakerFilteredResult {
        result: filtered_result,
        optimistic_result,
        speaker_info_present,
        stable_non_target_utterance_present: stable_same_cluster_owner_absence_present,
        target_speech_end_ms,
        wake_target_speech_end_ms: observed_wake_speaker_end_ms.or(prior_wake_speaker_end_ms),
        stable_attributed_speech_end_ms,
        pending_unattributed_text,
    }
}

fn provider_response_metadata(
    json: &Value,
    result: &Value,
    has_final_frame: bool,
    authoritative_two_pass: bool,
) -> Value {
    let mut sources = Vec::new();
    let utterance_count =
        result
            .get("utterances")
            .and_then(Value::as_array)
            .map_or(0, |utterances| {
                for utterance in utterances {
                    let Some(source) = utterance
                        .get("additions")
                        .and_then(|additions| additions.get("source"))
                        .and_then(Value::as_str)
                    else {
                        continue;
                    };
                    if !sources.iter().any(|existing| *existing == source) {
                        sources.push(source);
                    }
                }
                utterances.len()
            });
    // Keep enough structure to diagnose diarization boundary regressions
    // without ever writing transcript content to disk. Session 1705 could only
    // be narrowed to a cloud cluster handoff from aggregate character counts;
    // final speaker/timing metadata makes the next occurrence machine-auditable.
    let final_speaker_timeline = has_final_frame.then(|| {
        result
            .get("utterances")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|utterance| {
                json!({
                    "speaker_id": utterance_speaker_id(utterance),
                    "start_ms": utterance_start_ms(utterance),
                    "end_ms": utterance_end_ms(utterance),
                    "chars": utterance
                        .get("text")
                        .and_then(Value::as_str)
                        .map_or(0, |text| text.chars().count()),
                    "stable": utterance_is_stable(utterance),
                })
            })
            .collect::<Vec<_>>()
    });
    json!({
        "audio_duration_ms": server_audio_duration_ms(json),
        "sources": sources,
        "utterance_count": utterance_count,
        "has_final_frame": has_final_frame,
        "authoritative_two_pass": authoritative_two_pass,
        "two_pass_empty": result_marks_two_pass_empty(result),
        "provider_result_chars": normalized_result(json)
            .and_then(|provider_result| provider_result.get("text"))
            .and_then(Value::as_str)
            .map_or(0, |text| text.chars().count()),
        "result_chars": result
            .get("text")
            .and_then(Value::as_str)
            .map_or(0, |text| text.chars().count()),
        "final_speaker_timeline": final_speaker_timeline,
    })
}

pub struct VolcengineStreamingASR {
    credentials: VolcengineCredentials,
    hotwords: Vec<DictionaryHotword>,
    session_options: VolcengineSessionOptions,
    state: ParkingMutex<SyncState>,
    partial_callback: ParkingMutex<Option<PartialTranscriptCallback>>,
    visual_partial_callback: ParkingMutex<Option<VisualPartialTranscriptCallback>>,
    final_intermediate_callback: ParkingMutex<Option<FinalIntermediateTranscriptCallback>>,
    target_speaker_update_callback: ParkingMutex<Option<TargetSpeakerUpdateCallback>>,
    streaming_event_callback: ParkingMutex<Option<StreamingEventCallback>>,
    /// Guards the WebSocket write half so concurrent `send` calls serialize.
    /// Stored as Arc so spawned send tasks can hold their own clone — independent
    /// of the lifetime of any particular `&self` borrow.
    writer: SharedWriter,
    final_rx: ParkingMutex<Option<oneshot::Receiver<Result<RawTranscript, VolcengineASRError>>>>,
    /// 单 worker 模式：consume_pcm_chunk 把 (seq, chunk) 入队这个 channel，
    /// open_session 里 spawn 出的唯一 worker 串行 recv + send_binary，
    /// 保证 seq 顺序严格等于实际发送顺序。session 结束时 take() 掉这个 sender，
    /// worker 的 recv() 返回 None 自动退出。
    audio_tx: ParkingMutex<Option<mpsc::UnboundedSender<(i32, Vec<u8>)>>>,
    /// 队列里 + worker 在飞的 audio 帧总数。consume +N，worker send 完一帧 -1。
    /// send_last_frame 必须等它降到 0 才能安全发末帧，否则末帧可能被服务端先收到
    /// 而把后续 chunk 当成「stream 已结束」之后的多余数据丢弃 → 尾句丢失。
    pending_sends: Arc<AtomicUsize>,
    /// Session 内排队或正在写入 WebSocket 的音频帧峰值，用于验证消费者是否持续跟上
    /// 100 ms 一帧的生产速度；不记录任何音频内容。
    pending_sends_high_water: Arc<AtomicUsize>,
    send_done: Arc<Notify>,
    audio_delivery_changed: Arc<Notify>,
    final_frame_result: OnceCell<Result<(), VolcengineASRError>>,
    /// Full normalized session audio retained on the host for one exceptional
    /// replay when the live WebSocket transport fails. At 32 KiB/s this stays
    /// small for dictation sessions and does not affect device memory.
    retained_pcm: ParkingMutex<Vec<u8>>,
    recovery_replay_started: AtomicBool,
    #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
    target_speaker_stream:
        ParkingMutex<Option<Arc<super::target_speaker_extraction::TargetSpeakerStream>>>,
}

#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
fn target_speaker_final_required(
    physical_interference_detected: bool,
    sustained_non_target_seen: bool,
    degraded_owner_tail_seen: bool,
) -> bool {
    physical_interference_detected || sustained_non_target_seen || degraded_owner_tail_seen
}

#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
fn degraded_owner_tail_suggests_interference(evidence: &[LocalSpeakerEvidence]) -> bool {
    const TAIL_WINDOWS: usize = 4;
    const MIN_BASELINE_SCORE: f32 = 0.60;
    const REPEATED_DROP: f32 = 0.12;
    const SEVERE_DROP: f32 = 0.18;
    const MIN_DEGRADED_WINDOWS: usize = 3;

    if evidence.len() < TAIL_WINDOWS + 3 {
        return false;
    }
    let tail_start = evidence.len() - TAIL_WINDOWS;
    let baseline_peak = evidence[..tail_start]
        .iter()
        .map(|sample| sample.classification.score())
        .fold(0.0f32, f32::max);
    if baseline_peak < MIN_BASELINE_SCORE {
        return false;
    }
    let tail = &evidence[tail_start..];
    let degraded = tail
        .iter()
        .filter(|sample| sample.classification.score() + REPEATED_DROP <= baseline_peak)
        .count();
    let severe = tail
        .iter()
        .any(|sample| sample.classification.score() + SEVERE_DROP <= baseline_peak);
    degraded >= MIN_DEGRADED_WINDOWS && severe
}

#[derive(Clone)]
struct RecoverySpeakerSnapshot {
    local_audio_duration_ms: Option<u64>,
    local_speech_end_ms: Option<u64>,
    local_target_speech_end_ms: Option<u64>,
    local_non_target_speech_end_ms: Option<u64>,
    local_sustained_non_target_speech_end_ms: Option<u64>,
    local_speaker_classification: Option<crate::speaker_verification::SessionSpeakerClassification>,
    local_speaker_tracking_enabled: bool,
    local_wake_owner_verified: bool,
    local_speaker_profile_adaptive: bool,
    local_speaker_stable_target: bool,
    local_target_confirmed: bool,
    local_consecutive_target: u8,
    local_consecutive_non_target: u8,
    local_consecutive_transcript_hard_non_target: u8,
    local_consecutive_strong_non_target: u8,
    local_owner_absence_run_started_ms: Option<u64>,
    local_owner_absence_run_confirmed: bool,
    local_speaker_evidence: Vec<LocalSpeakerEvidence>,
    wake_speaker_phrase: Option<String>,
    wake_target_speech_end_ms: Option<u64>,
    owner_isolation_frozen: bool,
    owner_isolation_ceiling_text: String,
    owner_isolation_ceiling_segments: Vec<TranscriptSegment>,
}

impl VolcengineStreamingASR {
    pub fn new(credentials: VolcengineCredentials, hotwords: Vec<DictionaryHotword>) -> Self {
        Self::new_with_session_options(credentials, hotwords, VolcengineSessionOptions::default())
    }

    pub fn new_with_session_options(
        credentials: VolcengineCredentials,
        hotwords: Vec<DictionaryHotword>,
        session_options: VolcengineSessionOptions,
    ) -> Self {
        Self {
            credentials,
            hotwords,
            session_options,
            state: ParkingMutex::new(SyncState::default()),
            partial_callback: ParkingMutex::new(None),
            visual_partial_callback: ParkingMutex::new(None),
            final_intermediate_callback: ParkingMutex::new(None),
            target_speaker_update_callback: ParkingMutex::new(None),
            streaming_event_callback: ParkingMutex::new(None),
            writer: Arc::new(AsyncMutex::new(None)),
            final_rx: ParkingMutex::new(None),
            audio_tx: ParkingMutex::new(None),
            pending_sends: Arc::new(AtomicUsize::new(0)),
            pending_sends_high_water: Arc::new(AtomicUsize::new(0)),
            send_done: Arc::new(Notify::new()),
            audio_delivery_changed: Arc::new(Notify::new()),
            final_frame_result: OnceCell::new(),
            retained_pcm: ParkingMutex::new(Vec::new()),
            recovery_replay_started: AtomicBool::new(false),
            #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
            target_speaker_stream: ParkingMutex::new(None),
        }
    }

    #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
    pub fn start_target_speaker_extraction(&self, wake_pcm: &[u8], wake_end_seconds: f32) {
        let enrollment = super::target_speaker_extraction::wake_phrase_enrollment_pcm(
            wake_pcm,
            wake_end_seconds,
        );
        if enrollment.len() < 16_000 {
            log::warn!(
                "[target-speaker] owner-only stream skipped: enrollment_ms={}",
                enrollment.len() / 32
            );
            return;
        }
        let mut slot = self.target_speaker_stream.lock();
        if slot.is_some() {
            return;
        }
        *slot = Some(
            super::target_speaker_extraction::TargetSpeakerStream::start(
                self.credentials.clone(),
                self.hotwords.clone(),
                enrollment,
            ),
        );
        log::info!(
            "[target-speaker] owner-only stream armed model_sha256={}",
            super::target_speaker_extraction::MODEL_SHA256
        );
    }

    #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
    pub async fn await_target_speaker_final(&self) -> Result<Option<RawTranscript>, String> {
        let stream = self.target_speaker_stream.lock().take();
        let Some(stream) = stream else {
            return Ok(None);
        };
        let (sustained_non_target_seen, degraded_owner_tail_seen) = {
            let state = self.state.lock();
            (
                state.local_sustained_non_target_speech_end_ms.is_some(),
                degraded_owner_tail_suggests_interference(&state.local_speaker_evidence),
            )
        };
        let physical_interference_detected = stream.interference_detected();
        if !target_speaker_final_required(
            physical_interference_detected,
            sustained_non_target_seen,
            degraded_owner_tail_seen,
        ) {
            stream.cancel();
            log::info!(
                "[target-speaker] no physical or explicit non-owner evidence; preserving low-latency primary final"
            );
            return Ok(None);
        }
        log::info!(
            "[target-speaker] awaiting owner-only final physical_overlap={} sustained_non_target={} degraded_owner_tail={}",
            physical_interference_detected,
            sustained_non_target_seen,
            degraded_owner_tail_seen
        );
        match tokio::time::timeout(Duration::from_secs(12), stream.finish()).await {
            Ok(result) => result,
            Err(_) => {
                stream.cancel();
                Err("target-speaker owner-only stream timed out".to_string())
            }
        }
    }

    fn recovery_speaker_snapshot(&self) -> RecoverySpeakerSnapshot {
        let state = self.state.lock();
        RecoverySpeakerSnapshot {
            local_audio_duration_ms: state.local_audio_duration_ms,
            local_speech_end_ms: state.local_speech_end_ms,
            local_target_speech_end_ms: state.local_target_speech_end_ms,
            local_non_target_speech_end_ms: state.local_non_target_speech_end_ms,
            local_sustained_non_target_speech_end_ms: state
                .local_sustained_non_target_speech_end_ms,
            local_speaker_classification: state.local_speaker_classification.clone(),
            local_speaker_tracking_enabled: state.local_speaker_tracking_enabled,
            local_wake_owner_verified: state.local_wake_owner_verified,
            local_speaker_profile_adaptive: state.local_speaker_profile_adaptive,
            local_speaker_stable_target: state.local_speaker_stable_target,
            local_target_confirmed: state.local_target_confirmed,
            local_consecutive_target: state.local_consecutive_target,
            local_consecutive_non_target: state.local_consecutive_non_target,
            local_consecutive_transcript_hard_non_target: state
                .local_consecutive_transcript_hard_non_target,
            local_consecutive_strong_non_target: state.local_consecutive_strong_non_target,
            local_owner_absence_run_started_ms: state.local_owner_absence_run_started_ms,
            local_owner_absence_run_confirmed: state.local_owner_absence_run_confirmed,
            local_speaker_evidence: state.local_speaker_evidence.clone(),
            wake_speaker_phrase: state.wake_speaker_phrase.clone(),
            wake_target_speech_end_ms: state.wake_target_speech_end_ms,
            owner_isolation_frozen: state.owner_isolation_frozen,
            owner_isolation_ceiling_text: state.owner_isolation_ceiling_text.clone(),
            owner_isolation_ceiling_segments: state.owner_isolation_ceiling_segments.clone(),
        }
    }

    fn restore_recovery_speaker_snapshot(&self, snapshot: RecoverySpeakerSnapshot) {
        let mut state = self.state.lock();
        state.local_audio_duration_ms = snapshot.local_audio_duration_ms;
        state.local_speech_end_ms = snapshot.local_speech_end_ms;
        state.local_target_speech_end_ms = snapshot.local_target_speech_end_ms;
        state.local_non_target_speech_end_ms = snapshot.local_non_target_speech_end_ms;
        state.local_sustained_non_target_speech_end_ms =
            snapshot.local_sustained_non_target_speech_end_ms;
        state.local_speaker_classification = snapshot.local_speaker_classification;
        state.local_speaker_tracking_enabled = snapshot.local_speaker_tracking_enabled;
        state.local_wake_owner_verified = snapshot.local_wake_owner_verified;
        state.local_speaker_profile_adaptive = snapshot.local_speaker_profile_adaptive;
        state.local_speaker_stable_target = snapshot.local_speaker_stable_target;
        state.local_target_confirmed = snapshot.local_target_confirmed;
        state.local_consecutive_target = snapshot.local_consecutive_target;
        state.local_consecutive_non_target = snapshot.local_consecutive_non_target;
        state.local_consecutive_transcript_hard_non_target =
            snapshot.local_consecutive_transcript_hard_non_target;
        state.local_consecutive_strong_non_target = snapshot.local_consecutive_strong_non_target;
        state.local_owner_absence_run_started_ms = snapshot.local_owner_absence_run_started_ms;
        state.local_owner_absence_run_confirmed = snapshot.local_owner_absence_run_confirmed;
        state.local_speaker_evidence = snapshot.local_speaker_evidence;
        state.wake_speaker_phrase = snapshot.wake_speaker_phrase;
        state.wake_target_speech_end_ms = snapshot.wake_target_speech_end_ms;
        state.owner_isolation_frozen = snapshot.owner_isolation_frozen;
        state.owner_isolation_ceiling_text = snapshot.owner_isolation_ceiling_text;
        state.owner_isolation_ceiling_segments = snapshot.owner_isolation_ceiling_segments;
    }

    pub async fn replay_retained_audio_once(&self) -> Result<RawTranscript, VolcengineASRError> {
        let (pcm, speaker_snapshot) = self.claim_retained_audio_for_replay()?;
        self.replay_claimed_audio(pcm, speaker_snapshot, "exact", 1.0)
            .await
    }

    pub async fn replay_retained_audio_once_for_empty_final(
        &self,
    ) -> Result<RawTranscript, VolcengineASRError> {
        let (pcm, speaker_snapshot) = self.claim_retained_audio_for_replay()?;
        let (pcm, gain, peak_before, peak_after) = bounded_empty_final_replay_pcm(&pcm);
        log::warn!(
            "[asr] empty-final recovery representation pcm_bytes={} gain={gain:.4} peak_before={peak_before} peak_after={peak_after}",
            pcm.len()
        );
        self.replay_claimed_audio(pcm, speaker_snapshot, "empty-final", gain)
            .await
    }

    async fn replay_claimed_audio(
        &self,
        pcm: Vec<u8>,
        speaker_snapshot: RecoverySpeakerSnapshot,
        reason: &'static str,
        gain: f64,
    ) -> Result<RawTranscript, VolcengineASRError> {
        let replay = Arc::new(Self::new_with_session_options(
            self.credentials.clone(),
            self.hotwords.clone(),
            self.session_options,
        ));
        log::warn!(
            "[asr] starting one bounded full-audio recovery replay reason={reason} gain={gain:.4} pcm_bytes={} audio_ms={}",
            pcm.len(),
            pcm.len() as u64 / 32
        );
        replay.open_session().await?;
        replay.restore_recovery_speaker_snapshot(speaker_snapshot);
        replay.mark_audio_delivery_ready();
        replay.consume_pcm_chunk(&pcm);
        replay.send_last_frame().await?;
        let result = replay.await_final_result().await;
        if result.is_ok() {
            log::info!("[asr] full-audio recovery replay completed successfully");
        }
        result
    }

    fn claim_retained_audio_for_replay(
        &self,
    ) -> Result<(Vec<u8>, RecoverySpeakerSnapshot), VolcengineASRError> {
        if self.recovery_replay_started.swap(true, Ordering::SeqCst) {
            return Err(VolcengineASRError::ConnectionFailed(
                "full-audio recovery replay already attempted".into(),
            ));
        }
        let pcm = self.retained_pcm.lock().clone();
        if pcm.is_empty() {
            return Err(VolcengineASRError::ConnectionFailed(
                "full-audio recovery replay has no retained PCM".into(),
            ));
        }
        let speaker_snapshot = self.recovery_speaker_snapshot();
        Ok((pcm, speaker_snapshot))
    }

    /// F2 云端空转判定：终稿为空时，本地 VAD 证据显示用户整段都在说话
    /// （人声尾端正点贴近本地音频总长）且音频足量。命中时才允许一次
    /// retained-audio 重试；纯静音 / 仅唤醒词残段不触发。
    pub fn has_sustained_local_speech_evidence(&self) -> bool {
        let state = self.state.lock();
        let (Some(speech_end_ms), Some(audio_duration_ms)) =
            (state.local_speech_end_ms, state.local_audio_duration_ms)
        else {
            return false;
        };
        audio_duration_ms >= EMPTY_SPIN_MIN_LOCAL_AUDIO_MS
            && speech_end_ms + EMPTY_SPIN_SPEECH_TAIL_SLACK_MS >= audio_duration_ms
    }

    /// Physical/manual short dictation recovery gate. The coordinator keeps
    /// this out of automatic-wake sessions so a wake phrase by itself cannot
    /// be amplified into fabricated body text.
    pub fn has_local_speech_evidence(&self) -> bool {
        let state = self.state.lock();
        state.local_audio_duration_ms.unwrap_or(0) >= EMPTY_SPIN_MIN_LOCAL_AUDIO_MS
            && state.local_speech_end_ms.is_some()
    }

    /// Memory-only copy of the exact PCM already sent to this session. The
    /// coordinator uses it for a bounded, local shadow decode while the cloud
    /// final is in flight. Nothing is persisted and the session remains the
    /// sole owner of the authoritative provider transcript.
    pub fn retained_pcm_snapshot(&self) -> Vec<u8> {
        self.retained_pcm.lock().clone()
    }

    /// A shadow decode may help recover cloud omissions only for a wake-owned
    /// session that has never produced evidence of another speaker. Once local
    /// isolation has seen a non-target boundary, the safer behavior is to keep
    /// the already-filtered cloud ledger and never reintroduce text from the
    /// mixed waveform.
    pub fn may_run_local_shadow_decode(&self) -> bool {
        let state = self.state.lock();
        state.local_speaker_tracking_enabled
            && state.local_wake_owner_verified
            && state.local_non_target_speech_end_ms.is_none()
            && state.local_sustained_non_target_speech_end_ms.is_none()
            && !state.owner_isolation_frozen
    }

    pub fn permits_local_shadow_omission_recovery(&self) -> bool {
        let state = self.state.lock();
        let owner_only = state.local_speaker_tracking_enabled
            && state.local_wake_owner_verified
            && state.local_target_confirmed
            && state.local_non_target_speech_end_ms.is_none()
            && state.local_sustained_non_target_speech_end_ms.is_none()
            && !state.owner_isolation_frozen;
        let owner_end_aligned = state
            .target_speech_end_ms
            .zip(state.local_target_speech_end_ms)
            .is_some_and(|(cloud_end, local_end)| {
                cloud_end.abs_diff(local_end) <= LOCAL_SHADOW_OWNER_END_ALIGNMENT_MS
            });
        owner_only && owner_end_aligned
    }

    pub fn is_connected(&self) -> bool {
        self.state.lock().is_connected
    }

    fn set_audio_delivery_readiness(&self, next: AudioDeliveryReadiness) {
        let changed = {
            let mut state = self.state.lock();
            if matches!(
                &state.audio_delivery_readiness,
                AudioDeliveryReadiness::Cancelled
            ) && !matches!(&next, AudioDeliveryReadiness::Cancelled)
            {
                false
            } else {
                state.audio_delivery_readiness = next;
                true
            }
        };
        if changed {
            self.audio_delivery_changed.notify_waiters();
        }
    }

    fn mark_audio_delivery_opening(&self) {
        self.set_audio_delivery_readiness(AudioDeliveryReadiness::Opening);
    }

    pub fn mark_audio_delivery_ready(&self) {
        self.set_audio_delivery_readiness(AudioDeliveryReadiness::Ready);
    }

    pub(crate) fn mark_audio_delivery_failed(&self, error: VolcengineASRError) {
        self.set_audio_delivery_readiness(AudioDeliveryReadiness::Failed(error));
    }

    fn mark_audio_delivery_cancelled(&self) {
        self.set_audio_delivery_readiness(AudioDeliveryReadiness::Cancelled);
    }

    async fn await_audio_delivery_ready(
        &self,
        timeout: Duration,
    ) -> Result<(), VolcengineASRError> {
        let deadline = Instant::now() + timeout;
        loop {
            // Create the waiter before inspecting state. notify_waiters() then
            // cannot lose a concurrent bridge-flush or startup-failure wakeup.
            let notified = self.audio_delivery_changed.notified();
            match self.state.lock().audio_delivery_readiness.clone() {
                AudioDeliveryReadiness::Ready => return Ok(()),
                AudioDeliveryReadiness::Failed(error) => return Err(error),
                AudioDeliveryReadiness::Cancelled => {
                    return Err(VolcengineASRError::ConnectionFailed(
                        "audio delivery startup was cancelled".into(),
                    ));
                }
                AudioDeliveryReadiness::Opening => {}
            }

            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(VolcengineASRError::ConnectionFailed(format!(
                    "audio delivery startup did not reach ready state within {} ms",
                    timeout.as_millis()
                )));
            }
            tokio::time::timeout(remaining, notified)
                .await
                .map_err(|_| {
                    VolcengineASRError::ConnectionFailed(format!(
                        "audio delivery startup did not reach ready state within {} ms",
                        timeout.as_millis()
                    ))
                })?;
        }
    }

    async fn await_audio_delivery_drained(
        &self,
        timeout: Duration,
    ) -> Result<(), VolcengineASRError> {
        let deadline = Instant::now() + timeout;
        loop {
            // Both futures must exist before inspecting state so a concurrent
            // worker completion or failure cannot be lost between iterations.
            let delivery_changed = self.audio_delivery_changed.notified();
            let queue_drained = self.send_done.notified();
            match self.state.lock().audio_delivery_readiness.clone() {
                AudioDeliveryReadiness::Failed(error) => return Err(error),
                AudioDeliveryReadiness::Cancelled => {
                    return Err(VolcengineASRError::ConnectionFailed(
                        "audio delivery was cancelled before the final frame".into(),
                    ));
                }
                AudioDeliveryReadiness::Ready if self.pending_sends.load(Ordering::SeqCst) == 0 => {
                    return Ok(());
                }
                AudioDeliveryReadiness::Opening | AudioDeliveryReadiness::Ready => {}
            }

            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(VolcengineASRError::ConnectionFailed(format!(
                    "websocket audio delivery drain did not complete within {} ms (pending_frames={})",
                    timeout.as_millis(),
                    self.pending_sends.load(Ordering::SeqCst)
                )));
            }
            tokio::time::timeout(remaining, async {
                tokio::select! {
                    _ = delivery_changed => {},
                    _ = queue_drained => {},
                }
            })
            .await
            .map_err(|_| {
                VolcengineASRError::ConnectionFailed(format!(
                    "websocket audio delivery drain did not complete within {} ms (pending_frames={})",
                    timeout.as_millis(),
                    self.pending_sends.load(Ordering::SeqCst)
                ))
            })?;
        }
    }

    pub fn set_partial_transcript_callback(
        &self,
        callback: Option<Arc<dyn Fn(String) + Send + Sync>>,
    ) {
        *self.partial_callback.lock() = callback;
    }

    pub fn set_visual_partial_transcript_callback(
        &self,
        callback: Option<Arc<dyn Fn(String) + Send + Sync>>,
    ) {
        *self.visual_partial_callback.lock() = callback;
    }

    pub fn set_final_intermediate_transcript_callback(
        &self,
        callback: Option<FinalIntermediateTranscriptCallback>,
    ) {
        *self.final_intermediate_callback.lock() = callback;
    }

    pub fn set_target_speaker_update_callback(
        &self,
        callback: Option<TargetSpeakerUpdateCallback>,
    ) {
        *self.target_speaker_update_callback.lock() = callback;
    }

    pub fn stable_target_speech_end_ms(&self) -> Option<u64> {
        self.state.lock().target_speech_end_ms
    }

    pub fn note_local_audio_activity(&self, audio_duration_ms: u64, speech_detected: bool) {
        let update = {
            let mut state = self.state.lock();
            state.local_audio_duration_ms = Some(
                state
                    .local_audio_duration_ms
                    .unwrap_or_default()
                    .max(audio_duration_ms),
            );
            if speech_detected {
                state.local_speech_end_ms = Some(
                    state
                        .local_speech_end_ms
                        .unwrap_or_default()
                        .max(audio_duration_ms),
                );
            }
            target_speaker_update_from_state(&state, false, false)
        };
        if update.speaker_id.is_some() || update.local_speaker_tracking_enabled {
            self.emit_target_speaker_update(update);
        }
    }

    pub fn note_local_speaker_classification(
        &self,
        audio_duration_ms: u64,
        classification: crate::speaker_verification::SessionSpeakerClassification,
    ) {
        let transcript_hard_non_target = matches!(
            classification,
            crate::speaker_verification::SessionSpeakerClassification::NonTarget { score }
                if score <= LOCAL_TRANSCRIPT_HARD_NON_TARGET_MAX_SCORE
        );
        self.note_local_speaker_observation(
            audio_duration_ms,
            classification,
            transcript_hard_non_target,
        );
    }

    pub fn note_local_speaker_observation(
        &self,
        audio_duration_ms: u64,
        classification: crate::speaker_verification::SessionSpeakerClassification,
        transcript_hard_non_target: bool,
    ) {
        let (update, stable_target, effective_classification, endpoint_strong_non_target) = {
            let mut state = self.state.lock();
            let endpoint_strong_non_target = matches!(
                classification,
                crate::speaker_verification::SessionSpeakerClassification::NonTarget { score }
                    if score <= LOCAL_ENDPOINT_STRONG_NON_TARGET_MAX_SCORE
            );
            let owner_absence_band =
                classification.score() <= LOCAL_ENDPOINT_OWNER_ABSENCE_CONTINUATION_MAX_SCORE;
            if endpoint_strong_non_target {
                if state.local_consecutive_strong_non_target == 0
                    && !state.local_owner_absence_run_confirmed
                {
                    state.local_owner_absence_run_started_ms = Some(audio_duration_ms);
                }
                state.local_consecutive_strong_non_target =
                    state.local_consecutive_strong_non_target.saturating_add(1);
                if state.local_consecutive_strong_non_target >= LOCAL_SPEAKER_SWITCH_CONFIRMATIONS {
                    state.local_owner_absence_run_confirmed = true;
                    state.local_non_target_speech_end_ms = Some(
                        state
                            .local_non_target_speech_end_ms
                            .unwrap_or_default()
                            .max(audio_duration_ms),
                    );
                }
            } else if state.local_owner_absence_run_confirmed && owner_absence_band {
                // Once two strong negatives agree, keep low-score Uncertain
                // windows in the same run. Session 752 produced this exact
                // shape while a second person continued speaking; the older
                // real-owner startup dip recovered above 0.30 immediately.
                state.local_consecutive_strong_non_target = 0;
                state.local_non_target_speech_end_ms = Some(
                    state
                        .local_non_target_speech_end_ms
                        .unwrap_or_default()
                        .max(audio_duration_ms),
                );
            } else {
                state.local_consecutive_strong_non_target = 0;
                state.local_owner_absence_run_started_ms = None;
                state.local_owner_absence_run_confirmed = false;
            }
            if state.local_owner_absence_run_confirmed
                && state
                    .local_owner_absence_run_started_ms
                    .is_some_and(|started_ms| {
                        audio_duration_ms.saturating_sub(started_ms)
                            >= LOCAL_ENDPOINT_OWNER_ABSENCE_MIN_RUN_MS
                    })
            {
                state.local_sustained_non_target_speech_end_ms = Some(
                    state
                        .local_sustained_non_target_speech_end_ms
                        .unwrap_or_default()
                        .max(audio_duration_ms),
                );
            }
            // Moderate cross-phrase negatives stay advisory: installed session
            // 2024 received 41 provider chars, but 0.174/0.172 owner scores used
            // to freeze the ledger and produce an empty final. Keep the new
            // extreme-mismatch channel independent from identity/endpoint
            // hysteresis and enable it only for a persisted enrolled owner.
            let transcript_hard_non_target_applies = transcript_hard_non_target
                && state.local_wake_owner_verified
                && !state.local_speaker_profile_adaptive;
            if transcript_hard_non_target_applies {
                state.local_consecutive_transcript_hard_non_target = state
                    .local_consecutive_transcript_hard_non_target
                    .saturating_add(1);
                if state.local_consecutive_transcript_hard_non_target
                    == LOCAL_SPEAKER_SWITCH_CONFIRMATIONS
                {
                    log::info!(
                        "[asr] confirmed extreme local mismatch kept non-destructive pending provider utterance boundary"
                    );
                }
            } else {
                state.local_consecutive_transcript_hard_non_target = 0;
                if !state.owner_isolation_frozen {
                    state.owner_isolation_ceiling_text.clear();
                    state.owner_isolation_ceiling_segments.clear();
                }
            }
            let effective_classification = match classification {
                crate::speaker_verification::SessionSpeakerClassification::NonTarget { score } => {
                    crate::speaker_verification::SessionSpeakerClassification::Uncertain { score }
                }
                other => other,
            };
            match effective_classification {
                crate::speaker_verification::SessionSpeakerClassification::Target { .. } => {
                    state.local_consecutive_target =
                        state.local_consecutive_target.saturating_add(1);
                    state.local_consecutive_non_target = 0;
                    if !state.local_speaker_stable_target
                        && state.local_consecutive_target >= LOCAL_SPEAKER_SWITCH_CONFIRMATIONS
                    {
                        state.local_speaker_stable_target = true;
                        unfreeze_owner_isolation_ledger(&mut state);
                    } else if state.owner_isolation_frozen
                        && state.local_consecutive_target >= LOCAL_SPEAKER_SWITCH_CONFIRMATIONS
                    {
                        // Provider/local dual-gate exclusion may freeze while
                        // the debounced identity still says owner. Two fresh
                        // high-confidence owner windows allow the real owner to
                        // resume after somebody else spoke.
                        unfreeze_owner_isolation_ledger(&mut state);
                    }
                }
                crate::speaker_verification::SessionSpeakerClassification::NonTarget { .. } => {
                    state.local_consecutive_non_target =
                        state.local_consecutive_non_target.saturating_add(1);
                    state.local_consecutive_target = 0;
                    if state.local_speaker_stable_target
                        && state.local_consecutive_non_target >= LOCAL_SPEAKER_SWITCH_CONFIRMATIONS
                    {
                        state.local_speaker_stable_target = false;
                        freeze_owner_isolation_ledger(&mut state);
                    }
                }
                crate::speaker_verification::SessionSpeakerClassification::Uncertain { .. } => {
                    state.local_consecutive_target = 0;
                    state.local_consecutive_non_target = 0;
                }
            }
            let stable_target = state.local_speaker_stable_target;
            if stable_target
                && matches!(
                    effective_classification,
                    crate::speaker_verification::SessionSpeakerClassification::Target { .. }
                )
            {
                state.local_target_confirmed = true;
            }
            // Endpoint clock is owner-only.
            // - Target（确信，≥0.55）：本人身份成立时才刷新。
            // - Uncertain（含 0.42–0.55 弱 Target 带）：不刷新也不冻结。
            //   2026-08-09 12:47:04：第二个人 0.426–0.58 的窗持续刷新时钟导致
            //   永不结束；本人说话中途变轻的覆盖由预览增长刷新兜底
            //   （refresh_local_target_from_owner_preview_activity）。
            // - NonTarget: do NOT refresh. Confident other-speaker windows must not
            //   lengthen auto-end while hysteresis still lags the true switch.
            // Growing Target-filtered ASR preview also refreshes the clock via
            // refresh_local_target_from_owner_preview_activity.
            if stable_target
                && state.local_target_confirmed
                && matches!(
                    effective_classification,
                    crate::speaker_verification::SessionSpeakerClassification::Target { .. }
                )
            {
                state.local_target_speech_end_ms = Some(
                    state
                        .local_target_speech_end_ms
                        .unwrap_or_default()
                        .max(audio_duration_ms),
                );
            }
            // Hysteresis describes the retained identity, not the newest
            // evidence window. A recovering Target (or Uncertain window) must
            // not advance the non-target endpoint clock before the second
            // Target restores the debounced identity.
            if !stable_target
                && matches!(
                    effective_classification,
                    crate::speaker_verification::SessionSpeakerClassification::NonTarget { .. }
                )
            {
                state.local_non_target_speech_end_ms = Some(
                    state
                        .local_non_target_speech_end_ms
                        .unwrap_or_default()
                        .max(audio_duration_ms),
                );
            }
            // Keep the raw newest sample for preview/endpoint gating. The
            // evidence timeline below receives the advisory classification so
            // this sample cannot remove committed provider text.
            state.local_speaker_classification = Some(classification);
            state.local_speaker_evidence.push(LocalSpeakerEvidence {
                audio_end_ms: audio_duration_ms,
                classification: effective_classification,
                stable_target,
            });
            if state.local_speaker_evidence.len() > LOCAL_SPEAKER_EVIDENCE_LIMIT {
                let overflow = state.local_speaker_evidence.len() - LOCAL_SPEAKER_EVIDENCE_LIMIT;
                state.local_speaker_evidence.drain(..overflow);
            }
            (
                target_speaker_update_from_state(&state, false, false),
                stable_target,
                effective_classification,
                endpoint_strong_non_target,
            )
        };
        log::info!(
            "[asr] local session-speaker evidence classification={classification:?} effective={effective_classification:?} stable_target={stable_target} endpoint_strong_non_target={endpoint_strong_non_target} transcript_hard_non_target={transcript_hard_non_target} audio_end_ms={audio_duration_ms}"
        );
        if update.speaker_id.is_some() || update.local_speaker_tracking_enabled {
            self.emit_target_speaker_update(update);
        }
    }

    pub fn note_local_speaker_profile_adaptive(&self, adaptive: bool) {
        let mut state = self.state.lock();
        if state.local_speaker_profile_adaptive == adaptive {
            return;
        }
        state.local_speaker_profile_adaptive = adaptive;
        log::info!(
            "[asr] local session-speaker profile mode adaptive={adaptive} (wake-derived profiles are advisory for final recovery)"
        );
    }

    pub fn note_local_speaker_tracking_started(&self, wake_phrase: &str) {
        self.note_local_speaker_tracking_started_with_owner(wake_phrase, false);
    }

    pub fn note_verified_local_speaker_tracking_started(&self, wake_phrase: &str) {
        self.note_local_speaker_tracking_started_with_owner(wake_phrase, true);
    }

    fn note_local_speaker_tracking_started_with_owner(
        &self,
        wake_phrase: &str,
        wake_owner_verified: bool,
    ) {
        let mut state = self.state.lock();
        state.local_speaker_tracking_enabled = true;
        state.local_wake_owner_verified = wake_owner_verified;
        state.local_speaker_profile_adaptive = false;
        state.local_speaker_stable_target = true;
        state.local_target_confirmed = false;
        state.local_consecutive_target = 0;
        state.local_consecutive_non_target = 0;
        state.local_consecutive_transcript_hard_non_target = 0;
        state.local_consecutive_strong_non_target = 0;
        state.local_owner_absence_run_started_ms = None;
        state.local_owner_absence_run_confirmed = false;
        state.local_sustained_non_target_speech_end_ms = None;
        state.local_speaker_evidence.clear();
        state.owner_isolation_frozen = false;
        state.owner_isolation_ceiling_text.clear();
        state.owner_isolation_ceiling_segments.clear();
        state.wake_speaker_phrase = Some(wake_phrase.to_string());
        state.wake_target_speech_end_ms = None;
        log::info!(
            "[asr] local session-speaker tracking anchored to wake speaker wake_owner_verified={wake_owner_verified}"
        );
    }

    pub(crate) fn set_streaming_event_callback(&self, callback: Option<StreamingEventCallback>) {
        *self.streaming_event_callback.lock() = callback;
    }

    fn emit_partial_transcript(&self, text: &str) {
        let callback = self.partial_callback.lock().clone();
        if let Some(callback) = callback {
            callback(text.to_string());
        }
    }

    fn emit_visual_partial_transcript(&self, text: &str) {
        let callback = self.visual_partial_callback.lock().clone();
        if let Some(callback) = callback {
            callback(text.to_string());
        }
    }

    fn emit_final_intermediate_transcript(&self, update: FinalIntermediateTranscript) {
        let callback = self.final_intermediate_callback.lock().clone();
        if let Some(callback) = callback {
            callback(update);
        }
    }

    fn emit_target_speaker_update(&self, update: TargetSpeakerUpdate) {
        let callback = self.target_speaker_update_callback.lock().clone();
        if let Some(callback) = callback {
            callback(update);
        }
    }

    fn emit_streaming_event(&self, event: VolcengineStreamingEvent) {
        let callback = self.streaming_event_callback.lock().clone();
        if let Some(callback) = callback {
            callback(event);
        }
    }

    pub async fn open_session(self: &Arc<Self>) -> Result<(), VolcengineASRError> {
        let result = self.open_session_for_deferred_audio().await;
        if let Err(error) = &result {
            self.mark_audio_delivery_failed(error.clone());
        }
        result
    }

    /// Opens the provider transport while leaving delivery readiness in
    /// `Opening` on failure. Coordinator paths that buffer capture in a
    /// `DeferredAsrBridge` use this variant so they can first move every
    /// buffered byte into `retained_pcm`, then publish the startup failure to
    /// a concurrent finalizer. This ordering prevents an empty recovery replay.
    pub(crate) async fn open_session_for_deferred_audio(
        self: &Arc<Self>,
    ) -> Result<(), VolcengineASRError> {
        self.mark_audio_delivery_opening();
        self.open_session_inner().await
    }

    async fn open_session_inner(self: &Arc<Self>) -> Result<(), VolcengineASRError> {
        if self.credentials.app_id.is_empty()
            || self.credentials.access_token.is_empty()
            || self.credentials.resource_id.is_empty()
        {
            return Err(VolcengineASRError::CredentialsMissing);
        }

        let connect_id = Uuid::new_v4().to_string();
        log::info!(
            "[asr] opening Volcengine {} session endpoint={} resource_id={} model=bigmodel enable_nonstream={} result_type={} end_window_size={:?} force_to_speech_time={:?}",
            self.session_options.endpoint.label(),
            self.session_options.endpoint.endpoint(),
            self.credentials.resource_id,
            self.session_options.enable_nonstream,
            self.session_options.result_type.as_str(),
            self.session_options.end_window_size_ms,
            self.session_options.force_to_speech_time_ms,
        );
        let mut request = self
            .session_options
            .endpoint
            .endpoint()
            .into_client_request()
            .map_err(|e| VolcengineASRError::ConnectionFailed(e.to_string()))?;
        let headers = request.headers_mut();
        headers.insert(
            "X-Api-App-Key",
            HeaderValue::from_str(&self.credentials.app_id)
                .map_err(|e| VolcengineASRError::ConnectionFailed(e.to_string()))?,
        );
        headers.insert(
            "X-Api-Access-Key",
            HeaderValue::from_str(&self.credentials.access_token)
                .map_err(|e| VolcengineASRError::ConnectionFailed(e.to_string()))?,
        );
        headers.insert(
            "X-Api-Resource-Id",
            HeaderValue::from_str(&self.credentials.resource_id)
                .map_err(|e| VolcengineASRError::ConnectionFailed(e.to_string()))?,
        );
        headers.insert(
            "X-Api-Connect-Id",
            HeaderValue::from_str(&connect_id)
                .map_err(|e| VolcengineASRError::ConnectionFailed(e.to_string()))?,
        );

        let (ws, _resp) = tokio::time::timeout(WEBSOCKET_CONNECT_TIMEOUT, connect_async(request))
            .await
            .map_err(|_| {
                VolcengineASRError::ConnectionFailed(format!(
                    "websocket connect timed out after {}s",
                    WEBSOCKET_CONNECT_TIMEOUT.as_secs()
                ))
            })?
            .map_err(classify_connect_error)?;
        let (write, read) = ws.split();

        let (tx, rx) = oneshot::channel();

        // Reset sync state for the new session.
        {
            let mut st = self.state.lock();
            // Automatic wake configures the verified owner anchor immediately
            // before opening the cloud stream. Preserve that pending-session
            // configuration across the transport reset: clearing
            // `local_wake_owner_verified` here made the strict owner ledger keep
            // the body provisional while the visual-only ledger also refused to
            // render it, leaving the capsule blank until provider settlement.
            let pending_speaker_anchor = PendingSessionSpeakerAnchor::capture(&st);
            st.pending_audio.clear();
            st.next_sequence = 1;
            st.bytes_sent = 0;
            st.frames_sent = 0;
            st.response_frames_seen = 0;
            st.partial_updates_seen = 0;
            st.is_connected = true;
            st.final_tx = Some(tx);
            st.runtime = Some(Handle::current());
            st.start = Some(Instant::now());
            st.finishing = false;
            st.last_partial_text.clear();
            st.best_transcript_text.clear();
            st.best_transcript_segments.clear();
            st.best_untimed_window.clear();
            st.optimistic_preview_text.clear();
            st.optimistic_preview_segments.clear();
            st.optimistic_untimed_window.clear();
            st.last_emitted_preview_text.clear();
            st.last_emitted_visual_preview_text.clear();
            st.last_server_audio_duration_ms = None;
            st.target_speaker_id = None;
            st.target_speech_end_ms = None;
            st.wake_target_speech_end_ms = None;
            st.stable_attributed_speech_end_ms = None;
            st.local_audio_duration_ms = None;
            st.local_speech_end_ms = None;
            st.local_target_speech_end_ms = None;
            st.local_non_target_speech_end_ms = None;
            st.local_sustained_non_target_speech_end_ms = None;
            st.local_speaker_classification = None;
            pending_speaker_anchor.restore_after_stream_reset(&mut st);
            st.local_target_confirmed = false;
            st.local_consecutive_target = 0;
            st.local_consecutive_non_target = 0;
            st.local_consecutive_transcript_hard_non_target = 0;
            st.local_consecutive_strong_non_target = 0;
            st.local_owner_absence_run_started_ms = None;
            st.local_owner_absence_run_confirmed = false;
            st.local_speaker_evidence.clear();
            st.owner_isolation_frozen = false;
            st.owner_isolation_ceiling_text.clear();
            st.owner_isolation_ceiling_segments.clear();
            st.speaker_info_present = false;
            st.pending_unattributed_text.clear();
            log::info!(
                "[asr] stream reset preserved pending speaker anchor local_tracking={} wake_owner_verified={} profile_adaptive={}",
                st.local_speaker_tracking_enabled,
                st.local_wake_owner_verified,
                st.local_speaker_profile_adaptive,
            );
        }
        self.pending_sends.store(0, Ordering::SeqCst);
        self.pending_sends_high_water.store(0, Ordering::SeqCst);
        *self.final_rx.lock() = Some(rx);
        *self.writer.lock().await = Some(write);

        // 起一个唯一的 audio worker：consume_pcm_chunk 把 (seq, chunk) 推到 audio_tx，
        // worker 这边 FIFO recv 然后串行 send_binary。session 结束后调用方
        // (cancel / handle_frame error / fallback_to_partial_or_error) 会 take 掉
        // self.audio_tx，channel 关闭，worker 自然退出。
        let (audio_tx, mut audio_rx) = mpsc::unbounded_channel::<(i32, Vec<u8>)>();
        *self.audio_tx.lock() = Some(audio_tx);
        let writer_for_worker = Arc::clone(&self.writer);
        let pending_for_worker = Arc::clone(&self.pending_sends);
        let notify_for_worker = Arc::clone(&self.send_done);
        let failed_delivery = Arc::downgrade(self);
        let role_label = self.session_options.endpoint.label();
        tokio::spawn(async move {
            while let Some((seq, chunk)) = audio_rx.recv().await {
                let frame = frame::build(
                    MessageType::AudioOnlyRequest,
                    Flags::PositiveSequence,
                    Serialization::None,
                    &chunk,
                    Some(seq),
                );
                if let Err(error) = send_binary(&writer_for_worker, frame).await {
                    log::error!(
                        "[asr] {} audio frame seq={} send 失败: {}",
                        role_label,
                        seq,
                        error
                    );
                    if let Some(asr) = failed_delivery.upgrade() {
                        asr.mark_audio_delivery_failed(error);
                        *asr.audio_tx.lock() = None;
                    }
                    if pending_for_worker.fetch_sub(1, Ordering::SeqCst) == 1 {
                        notify_for_worker.notify_waiters();
                    }
                    break;
                }
                if pending_for_worker.fetch_sub(1, Ordering::SeqCst) == 1 {
                    notify_for_worker.notify_waiters();
                }
            }
        });
        // Send the first frame: full client request with seq=1.
        let payload_json = self.build_first_frame_payload(&connect_id);
        let payload_bytes = serde_json::to_vec(&payload_json)
            .map_err(|e| VolcengineASRError::DecodeFailed(e.to_string()))?;
        let first_seq = self.allocate_positive_seq();
        let frame = frame::build(
            MessageType::FullClientRequest,
            Flags::PositiveSequence,
            Serialization::Json,
            &payload_bytes,
            Some(first_seq),
        );
        send_binary(&self.writer, frame).await?;

        // Spawn the receive loop. Holds a Weak<Self> so it doesn't keep
        // the struct alive forever if callers drop their Arcs.
        let weak_self = Arc::downgrade(self);
        tokio::spawn(async move {
            let mut read = read;
            while let Some(msg) = read.next().await {
                let Some(this) = weak_self.upgrade() else {
                    break;
                };
                match msg {
                    Ok(Message::Binary(data)) => {
                        if !this.handle_frame(&data) {
                            break;
                        }
                    }
                    Ok(Message::Close(_)) => {
                        // 服务端没发 final 就关连接 → 用最近一次 partial 兜底，不丢已识别的文字。
                        this.fallback_to_partial_or_error(VolcengineASRError::NoFinalResult);
                        break;
                    }
                    Ok(_) => { /* ignore text/ping/pong */ }
                    Err(e) => {
                        log::error!("[asr] receive loop error: {}", e);
                        // 网络中断同样回退到 partial，让用户至少拿到已经识别的部分。
                        this.fallback_to_partial_or_error(VolcengineASRError::ConnectionFailed(
                            e.to_string(),
                        ));
                        break;
                    }
                }
                if !this.state.lock().is_connected {
                    break;
                }
            }
        });

        Ok(())
    }

    pub async fn send_last_frame(&self) -> Result<(), VolcengineASRError> {
        self.final_frame_result
            .get_or_init(|| self.send_last_frame_once())
            .await
            .clone()
    }

    async fn send_last_frame_once(&self) -> Result<(), VolcengineASRError> {
        // Seal immediately so a proactive endpoint finalization cannot race
        // later firmware-drain PCM into the stream after its negative frame.
        self.state.lock().finishing = true;
        let delivery_ready_started = Instant::now();
        self.await_audio_delivery_ready(FINAL_FRAME_SEND_BUDGET)
            .await?;
        log::info!(
            "[asr] final audio delivery readiness elapsed_ms={}",
            delivery_ready_started.elapsed().as_millis()
        );
        // The negative end frame is only legal after every positive audio
        // sequence reaches the socket. A fixed short wait can emit the end
        // out of order and corrupt the provider sequence, so scale the bound
        // from actual queued 100 ms audio frames instead.
        let queued_frames = self.pending_sends.load(Ordering::SeqCst);
        let drain_budget = final_audio_drain_budget(queued_frames);
        let drain_started = Instant::now();
        self.await_audio_delivery_drained(drain_budget).await?;
        log::info!(
            "[asr] final audio delivery drained queued_frames={} elapsed_ms={} budget_ms={}",
            queued_frames,
            drain_started.elapsed().as_millis(),
            drain_budget.as_millis()
        );
        // Drain leftover audio (if any) into one final positive-sequence frame.
        let finish_send_deadline = Instant::now() + FINAL_FRAME_SEND_BUDGET;
        let leftover = {
            let mut st = self.state.lock();
            if st.pending_audio.is_empty() {
                None
            } else {
                Some(std::mem::take(&mut st.pending_audio))
            }
        };

        if let Some(buf) = leftover {
            let seq = self.allocate_positive_seq();
            let len = buf.len();
            let frame = frame::build(
                MessageType::AudioOnlyRequest,
                Flags::PositiveSequence,
                Serialization::None,
                &buf,
                Some(seq),
            );
            {
                let mut st = self.state.lock();
                st.bytes_sent += len;
                st.frames_sent += 1;
            }
            self.send_finish_frame(frame, finish_send_deadline).await?;
        }

        // Final frame: negativeSequence + negative seq number signals stream end.
        // 末帧用 negativeSequence + 负序号收尾，告诉服务端"流到此结束"。
        let final_seq = {
            let mut st = self.state.lock();
            let s = -st.next_sequence;
            st.next_sequence += 1;
            s
        };
        let frame = frame::build(
            MessageType::AudioOnlyRequest,
            Flags::NegativeSequence,
            Serialization::None,
            &[],
            Some(final_seq),
        );
        self.send_finish_frame(frame, finish_send_deadline).await?;

        let (total_bytes, total_frames, pending_high_water_frames) = {
            let st = self.state.lock();
            (
                st.bytes_sent,
                st.frames_sent,
                self.pending_sends_high_water.load(Ordering::SeqCst),
            )
        };
        let duration_ms = (total_bytes as f64 / BYTES_PER_MS) as u64;
        log::info!(
            "[asr] 发送总结：{} captured-audio frames, {} captured bytes (~{} ms), pending_send_high_water_frames={}",
            total_frames,
            total_bytes,
            duration_ms,
            pending_high_water_frames
        );
        Ok(())
    }

    async fn send_finish_frame(
        &self,
        frame: Vec<u8>,
        deadline: Instant,
    ) -> Result<(), VolcengineASRError> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(VolcengineASRError::ConnectionFailed(format!(
                "final frame send budget exhausted after {} ms",
                FINAL_FRAME_SEND_BUDGET.as_millis()
            )));
        }
        send_binary_with_timeout(
            &self.writer,
            frame,
            std::cmp::min(remaining, WEBSOCKET_SEND_TIMEOUT),
        )
        .await
    }

    pub async fn await_final_result(&self) -> Result<RawTranscript, VolcengineASRError> {
        self.await_final_result_with_timeout(FINAL_RESULT_TIMEOUT)
            .await
    }

    pub async fn await_final_result_with_timeout(
        &self,
        timeout: Duration,
    ) -> Result<RawTranscript, VolcengineASRError> {
        let rx = self.final_rx.lock().take();
        let Some(mut rx) = rx else {
            return Err(VolcengineASRError::NoFinalResult);
        };
        match tokio::time::timeout(timeout, &mut rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(VolcengineASRError::NoFinalResult),
            Err(_) => {
                if let Some((sent_audio_ms, transcript_end_ms)) = self.final_partial_coverage_gap()
                {
                    log::error!(
                        "[asr] final transcript coverage incomplete after full provider timeout: sent_audio_ms={sent_audio_ms} transcript_end_ms={transcript_end_ms}"
                    );
                    self.cancel();
                    return Err(VolcengineASRError::FinalResultCoverageIncomplete {
                        sent_audio_ms,
                        transcript_end_ms,
                    });
                }
                log::error!(
                    "[asr] provider final result timed out after {} ms",
                    timeout.as_millis()
                );
                self.cancel();
                Err(VolcengineASRError::FinalResultTimeout)
            }
        }
    }

    fn final_partial_coverage_gap(&self) -> Option<(u64, u64)> {
        final_partial_coverage_gap_from_state(&self.state.lock())
    }

    pub fn cancel(&self) {
        #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
        if let Some(stream) = self.target_speaker_stream.lock().take() {
            stream.cancel();
        }
        self.mark_audio_delivery_cancelled();
        let runtime = {
            let mut st = self.state.lock();
            st.is_connected = false;
            st.pending_audio.clear();
            st.runtime.clone()
        };
        // Drop audio sender → worker.recv() 返回 None → worker 退出，不再 hold writer。
        *self.audio_tx.lock() = None;
        if let Some(runtime) = runtime {
            // Close the writer asynchronously so the receive loop sees EOF.
            let writer = Arc::clone(&self.writer);
            runtime.spawn(async move {
                if let Some(mut w) = writer.lock().await.take() {
                    let _ = w.close().await;
                }
            });
        }
        // Wake an async final-result waiter without reporting user cancellation
        // as a provider error to the platform event sink.
        self.signal_error_silently(VolcengineASRError::NoFinalResult);
    }

    // ---- internals ----

    fn build_first_frame_payload(&self, connect_id: &str) -> Value {
        let mut request = json!({
            "model_name": "bigmodel",
            "enable_nonstream": self.session_options.enable_nonstream,
            // Seed ASR 2.0 exposes first-character acceleration for the
            // bidirectional stream. Keep it explicit so a provider-default
            // change cannot silently regress the capsule to sentence-only text.
            "enable_accelerate_text": true,
            "enable_itn": true,
            "enable_punc": true,
            "show_utterances": true,
            "enable_speaker_info": true,
            "result_type": self.session_options.result_type.as_str(),
        });
        if let Some(end_window_size_ms) = self.session_options.end_window_size_ms {
            request["end_window_size"] = Value::from(end_window_size_ms);
        }
        if let Some(force_to_speech_time_ms) = self.session_options.force_to_speech_time_ms {
            request["force_to_speech_time"] = Value::from(force_to_speech_time_ms);
        }
        if let Some(context) = hotword_context(&self.hotwords) {
            request["context"] = Value::String(context);
            let enabled_count = self.hotwords.iter().filter(|h| h.enabled).count();
            log::info!("[asr] hotwords injected: {}", enabled_count);
        }
        json!({
            "user": { "uid": connect_id },
            "audio": {
                "format": "pcm",
                "rate": 16000,
                "bits": 16,
                "channel": 1,
                "codec": "raw",
            },
            "request": request,
        })
    }

    fn allocate_positive_seq(&self) -> i32 {
        let mut st = self.state.lock();
        let s = st.next_sequence;
        st.next_sequence += 1;
        s
    }

    /// Returns `false` once the session has terminated (caller should stop reading).
    fn handle_frame(&self, data: &[u8]) -> bool {
        let Some(parsed) = frame::parse(data) else {
            log::error!("[asr] 帧解析失败 raw={}", hex_prefix(data, 32));
            return true;
        };

        if parsed.message_type == Some(MessageType::ErrorMessage) {
            let body = String::from_utf8_lossy(&parsed.payload).to_string();
            let code = parsed.error_code.unwrap_or(0);
            log::error!(
                "[asr] error frame code={} body={}",
                code,
                body.chars().take(200).collect::<String>()
            );
            self.fallback_to_partial_or_error(classify_provider_error(code, &body));
            self.state.lock().is_connected = false;
            *self.audio_tx.lock() = None;
            return false;
        }

        if parsed.message_type != Some(MessageType::FullServerResponse) {
            return true;
        }
        {
            let mut st = self.state.lock();
            st.response_frames_seen += 1;
        }

        let payload_for_log = std::str::from_utf8(&parsed.payload)
            .ok()
            .map(|payload| payload.chars().take(400).collect::<String>());

        let json: Value = match serde_json::from_slice(&parsed.payload) {
            Ok(v) => v,
            Err(_) => return true,
        };
        if let Some(duration_ms) = server_audio_duration_ms(&json) {
            let mut state = self.state.lock();
            state.last_server_audio_duration_ms = Some(
                state
                    .last_server_audio_duration_ms
                    .unwrap_or_default()
                    .max(duration_ms),
            );
        }
        let Some(result) = normalized_result(&json) else {
            return true;
        };

        let has_final = parsed.is_final();
        let (speaker_filtered_result, target_speaker_update, provisional_holds_endpoint) = {
            let mut state = self.state.lock();
            let local_speaker_tracking_enabled = state.local_speaker_tracking_enabled;
            let local_speaker_evidence = state.local_speaker_evidence.clone();
            let wake_speaker_phrase = state.wake_speaker_phrase.clone();
            let prior_wake_speaker_end_ms = state.wake_target_speech_end_ms;
            let wake_owner_verified = state.local_wake_owner_verified;
            let local_speaker_profile_adaptive = state.local_speaker_profile_adaptive;
            let confirmed_non_target_speech_end_ms = state.local_non_target_speech_end_ms;
            let filtered = filter_result_to_target_speaker_with_local_evidence_and_anchor(
                result,
                &mut state.target_speaker_id,
                local_speaker_tracking_enabled,
                &local_speaker_evidence,
                wake_speaker_phrase.as_deref(),
                prior_wake_speaker_end_ms,
                wake_owner_verified,
                local_speaker_profile_adaptive,
                confirmed_non_target_speech_end_ms,
            );
            if let Some(wake_end_ms) = filtered.wake_target_speech_end_ms {
                state.wake_target_speech_end_ms = Some(wake_end_ms);
            }
            if filtered.stable_non_target_utterance_present {
                freeze_owner_isolation_at_filtered_result(&mut state, &filtered.result);
            }
            let previous_end_ms = state.target_speech_end_ms;
            let previous_pending_text = std::mem::take(&mut state.pending_unattributed_text);
            if let Some(end_ms) = filtered.target_speech_end_ms {
                state.target_speech_end_ms = Some(previous_end_ms.unwrap_or_default().max(end_ms));
            }
            if let Some(end_ms) = filtered.stable_attributed_speech_end_ms {
                state.stable_attributed_speech_end_ms = Some(
                    state
                        .stable_attributed_speech_end_ms
                        .unwrap_or_default()
                        .max(end_ms),
                );
            }
            state.speaker_info_present |= filtered.speaker_info_present;
            let target_activity_advanced = state.target_speech_end_ms != previous_end_ms;
            let pending_unattributed_speech = !filtered.pending_unattributed_text.is_empty();
            let pending_activity_advanced = pending_unattributed_speech
                && filtered.pending_unattributed_text != previous_pending_text;
            state.pending_unattributed_text = filtered.pending_unattributed_text.clone();
            // Provisional body growth (installed 19df34c4: wake phrase stable,
            // body only in pending channel) may refresh the owner clock only
            // while local evidence is still Target. Room-speech provisional
            // tails must not keep auto-end open during NonTarget debounce.
            let provisional_holds_endpoint = pending_activity_advanced
                && local_speaker_allows_optimistic_preview(&state)
                && refresh_local_target_from_owner_preview_activity(&mut state);
            let update = target_speaker_update_from_state(
                &state,
                target_activity_advanced || provisional_holds_endpoint,
                pending_activity_advanced,
            );
            (filtered, update, provisional_holds_endpoint)
        };
        if provisional_holds_endpoint {
            log::info!(
                "[asr] provisional body growth refreshed local target endpoint clock local_target_end_ms={:?}",
                target_speaker_update.local_target_speech_end_ms
            );
        }
        if target_speaker_update.speaker_id.is_some() {
            log::info!(
                "[asr] target-speaker state speaker_id={:?} stable_end_ms={:?} audio_duration_ms={:?} provider_audio_duration_ms={:?} local_speech_end_ms={:?} local_target_end_ms={:?} local_non_target_end_ms={:?} local_tracking={} stable_attributed_end_ms={:?} pending_provisional={} target_advanced={} pending_advanced={}",
                target_speaker_update.speaker_id,
                target_speaker_update.target_speech_end_ms,
                target_speaker_update.audio_duration_ms,
                target_speaker_update.provider_audio_duration_ms,
                target_speaker_update.local_speech_end_ms,
                target_speaker_update.local_target_speech_end_ms,
                target_speaker_update.local_non_target_speech_end_ms,
                target_speaker_update.local_speaker_tracking_enabled,
                target_speaker_update.stable_attributed_speech_end_ms,
                target_speaker_update.pending_unattributed_speech,
                target_speaker_update.target_activity_advanced,
                target_speaker_update.pending_activity_advanced,
            );
        }
        self.emit_target_speaker_update(target_speaker_update);

        // The provider can split one uninterrupted owner into speaker 0 for
        // the wake phrase and speaker 1 for the body. The final path already
        // restores that sequential body when every overlapping local window
        // still owns the verified wake identity. Apply the same evidence to
        // live display: otherwise the provider has useful text for seconds,
        // but the capsule remains blank until the final packet (installed
        // session 239). This does not broaden committed output beyond the
        // existing final rule and still rejects overlap or any local
        // NonTarget evidence.
        let owner_safe_provider_split_preview =
            !has_final && !speaker_filtered_result.stable_non_target_utterance_present && {
                let target_text = speaker_filtered_result
                    .result
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let state = self.state.lock();
                sequential_speaker_split_gap_is_owner_safe(&state, result, target_text)
            };
        let pending_unattributed_speech = owner_safe_provider_split_preview
            || !speaker_filtered_result.pending_unattributed_text.is_empty();
        // A provider packet can first expose a safe target prefix while a
        // foreign/provisional tail is pending, then seal that tail as another
        // speaker. The target prefix is already in the protected session
        // ledger, but it was intentionally hidden during the provisional
        // packet. Publish it once the pending channel clears; otherwise the
        // later packet has no text delta and the capsule can stay blank.
        if !has_final
            && !pending_unattributed_speech
            && self
                .session_options
                .endpoint
                .emits_stream_preview_before_final()
        {
            let filtered_target_preview = speaker_filtered_result
                .result
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let settled_preview = {
                let mut state = self.state.lock();
                let best = if filtered_target_preview.trim().is_empty() {
                    state.best_transcript_text.clone()
                } else {
                    filtered_target_preview
                };
                let best_len = spoken_content_len(&best);
                let visible_len = spoken_content_len(&state.last_emitted_preview_text);
                if !best.trim().is_empty()
                    && best != state.last_emitted_preview_text
                    && best_len >= visible_len
                {
                    state.last_emitted_preview_text = best.clone();
                    state.partial_updates_seen += 1;
                    Some(best)
                } else {
                    None
                }
            };
            if let Some(preview) = settled_preview {
                log::info!(
                    "[asr] settled target preview published after provisional speaker tail chars={}",
                    preview.chars().count()
                );
                self.emit_partial_transcript(&preview);
                self.emit_streaming_event(VolcengineStreamingEvent::Partial(preview));
            }
        }
        // Cloud diarization can temporarily seal the verified owner's body as
        // another speaker while a newer raw tail is still provisional. The
        // strict owner ledger correctly withholds that tail, but withholding
        // every intermediate packet makes the capsule jump from (for example)
        // 22 to 56 characters several seconds later. Publish the growing raw
        // provider text through a dedicated visual-only callback while the
        // persisted wake identity remains debounced as owner and there is no
        // NonTarget/owner-absence evidence. This callback is deliberately not
        // the product partial callback: it cannot refresh endpoint clocks,
        // repair/fallback the final, or enter insertion.
        if !has_final
            && !owner_safe_provider_split_preview
            && self
                .session_options
                .endpoint
                .emits_stream_preview_before_final()
        {
            let visual_preview = {
                let mut state = self.state.lock();
                display_only_provisional_preview_candidate(
                    &mut state,
                    result,
                    pending_unattributed_speech,
                )
            };
            if let Some(preview) = visual_preview {
                log::info!(
                    "[asr] display-only provisional preview published chars={} endpoint_refresh=false final_ledger=false",
                    preview.chars().count()
                );
                self.emit_visual_partial_transcript(&preview);
            }
        }
        if !has_final
            && pending_unattributed_speech
            && {
                let state = self.state.lock();
                local_speaker_allows_optimistic_preview(&state)
            }
            && self
                .session_options
                .endpoint
                .emits_stream_preview_before_final()
        {
            let optimistic_candidate =
                transcript_candidate_from_result(if owner_safe_provider_split_preview {
                    result
                } else {
                    &speaker_filtered_result.optimistic_result
                });
            let optimistic_preview = {
                let mut state = self.state.lock();
                let (mut merged, segments, untimed_window) =
                    merge_streaming_candidate_with_untimed_window(
                        &state.optimistic_preview_text,
                        &state.optimistic_preview_segments,
                        &state.optimistic_untimed_window,
                        optimistic_candidate,
                    );
                merged = trim_repeated_short_streaming_tail(&merged);
                let owner_preview_allowed = local_speaker_allows_optimistic_preview(&state);
                let should_emit = owner_preview_allowed
                    && !is_unstable_initial_partial(&state.last_emitted_preview_text, &merged)
                    && !merged.trim().is_empty()
                    && state.last_emitted_preview_text != merged;
                if !merged.trim().is_empty() && owner_preview_allowed {
                    state.optimistic_preview_text = merged.clone();
                    state.optimistic_preview_segments = segments.clone();
                    state.optimistic_untimed_window = untimed_window;
                    // Only promote into best_transcript after the wake speaker is
                    // locally confirmed. Unlocked speakerless early text stays
                    // preview-only until that identity exists; session_committed
                    // still reads optimistic_preview for empty two_pass seals.
                    if state.local_speaker_tracking_enabled && state.local_target_confirmed {
                        commit_session_transcript_if_stronger(&mut state, &merged, segments);
                    }
                }
                if owner_safe_provider_split_preview {
                    // Once cloud A/B drift is corroborated by the retained wake
                    // identity, this text is no longer an unattributed tail.
                    // Keeping it pending blocked the 1 s owner endpoint even as
                    // the complete preview was already safe to display.
                    state.pending_unattributed_text.clear();
                }
                let preview_holds_endpoint = should_emit
                    && if owner_safe_provider_split_preview {
                        refresh_local_target_from_owner_safe_provider_split(&mut state)
                    } else {
                        refresh_local_target_from_owner_preview_activity(&mut state)
                    };
                if should_emit {
                    state.last_emitted_preview_text = merged.clone();
                    state.partial_updates_seen += 1;
                    Some((merged, preview_holds_endpoint))
                } else {
                    None
                }
            };
            if let Some((preview, preview_holds_endpoint)) = optimistic_preview {
                if owner_safe_provider_split_preview {
                    log::info!(
                        "[asr] owner-safe sequential provider split admitted to live preview chars={}",
                        preview.chars().count()
                    );
                }
                if preview_holds_endpoint {
                    let update = {
                        let state = self.state.lock();
                        target_speaker_update_from_state(&state, true, false)
                    };
                    log::info!(
                        "[asr] owner preview growth refreshed local target endpoint clock local_target_end_ms={:?}",
                        update.local_target_speech_end_ms
                    );
                    self.emit_target_speaker_update(update);
                }
                let elapsed_ms = self
                    .state
                    .lock()
                    .start
                    .map(|s| s.elapsed().as_millis() as u64)
                    .unwrap_or(0);
                log::info!(
                    "[asr] {} optimistic partial update chars={} elapsed_ms={}",
                    self.session_options.endpoint.label(),
                    preview.chars().count(),
                    elapsed_ms
                );
                self.emit_partial_transcript(&preview);
                self.emit_streaming_event(VolcengineStreamingEvent::Partial(preview));
            }
        }
        // Streaming partials may only see the first stable utterance
        // ("开始录音，那你") while result.text / optimistic already holds the
        // longer owner tail. On protocol final we must prefer the longer
        // owner-safe optimistic text when available — otherwise intermittent
        // mid-cut finals insert only two chars (installed 4ff44fc3).
        let prefer_final_provider_text =
            has_final && !speaker_filtered_result.stable_non_target_utterance_present && {
                let target_text = speaker_filtered_result
                    .result
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let state = self.state.lock();
                final_wake_only_provider_gap_is_owner_safe(&state, result, target_text)
                    || sequential_speaker_split_gap_is_owner_safe(&state, result, target_text)
                    || final_unsegmented_provider_tail_is_owner_safe(&state, result, target_text)
            };
        let prefer_final_optimistic = has_final
            && !prefer_final_provider_text
            && !speaker_filtered_result.stable_non_target_utterance_present
            && {
                let target_text = speaker_filtered_result
                    .result
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let optimistic_text = speaker_filtered_result
                    .optimistic_result
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let longer = spoken_content_len(optimistic_text) > spoken_content_len(target_text);
                let owner_ok = {
                    let state = self.state.lock();
                    local_speaker_allows_optimistic_preview(&state)
                        || local_speaker_allows_non_destructive_final_recovery(&state)
                        || spoken_content_len(&state.optimistic_preview_text)
                            >= spoken_content_len(optimistic_text)
                };
                longer && owner_ok
            };
        let result = if prefer_final_provider_text {
            log::info!(
                "[asr] protocol final restores owner-safe provider text after diarization regression target_chars={} provider_chars={}",
                speaker_filtered_result
                    .result
                    .get("text")
                    .and_then(Value::as_str)
                    .map(|t| t.chars().count())
                    .unwrap_or(0),
                result
                    .get("text")
                    .and_then(Value::as_str)
                    .map(|t| t.chars().count())
                    .unwrap_or(0)
            );
            result
        } else if prefer_final_optimistic {
            log::info!(
                "[asr] protocol final prefers longer owner-safe optimistic text target_chars={} optimistic_chars={}",
                speaker_filtered_result
                    .result
                    .get("text")
                    .and_then(Value::as_str)
                    .map(|t| t.chars().count())
                    .unwrap_or(0),
                speaker_filtered_result
                    .optimistic_result
                    .get("text")
                    .and_then(Value::as_str)
                    .map(|t| t.chars().count())
                    .unwrap_or(0)
            );
            &speaker_filtered_result.optimistic_result
        } else {
            &speaker_filtered_result.result
        };

        // 流结束信号只信帧头 flags（lastPacket / negativeSequence）。
        // 之前误把 utterance.definite=true 当成流结束——但那只代表"这一段语音已固化"，
        // 用户可能还在继续说。结果一收到第一个 definite=true 就关掉接收，
        // 后面用户讲的内容全部丢失（实测丢了 9 秒）。
        let candidate = transcript_candidate_from_result(result);
        let two_pass_empty_final = has_final && result_marks_two_pass_empty(result);
        let final_speaker_candidate_empty = has_final && candidate.text.trim().is_empty();
        let authoritative_two_pass = candidate.authoritative_cumulative && !two_pass_empty_final;
        let authoritative_final_candidate = (has_final && authoritative_two_pass)
            .then(|| (candidate.text.clone(), candidate.timed_segments.clone()));
        log::info!(
            "[asr] {} server metadata: {}",
            self.session_options.endpoint.label(),
            provider_response_metadata(&json, result, has_final, authoritative_two_pass)
        );
        if two_pass_empty_final {
            log::warn!(
                "[asr] {} protocol final is two_pass_empty seal-only; session speech ledger will not regress",
                self.session_options.endpoint.label()
            );
        }
        if let Some(payload) = payload_for_log {
            let text_is_empty = candidate.text.trim().is_empty();
            if text_is_empty && !has_final {
                log::debug!(
                    "[asr] {} server JSON(empty): {}",
                    self.session_options.endpoint.label(),
                    payload
                );
            } else {
                log::debug!(
                    "[asr] {} server JSON: {}",
                    self.session_options.endpoint.label(),
                    payload
                );
            }
        }
        if !has_final {
            let should_ignore = {
                let state = self.state.lock();
                is_unstable_initial_partial(&state.best_transcript_text, &candidate.text)
            };
            if should_ignore {
                log::debug!(
                    "[asr] ignored unstable initial partial: {}",
                    candidate.text.chars().take(20).collect::<String>()
                );
                return true;
            }
        }
        let (full_text, partial_changed, preview_holds_endpoint) = {
            let mut state = self.state.lock();
            let (mut merged, mut segments, untimed_window) =
                merge_streaming_candidate_with_untimed_window(
                    &state.best_transcript_text,
                    &state.best_transcript_segments,
                    &state.best_untimed_window,
                    candidate,
                );
            state.best_untimed_window = untimed_window;
            // A no-segment optimistic ledger can keep winning inside the
            // generic streaming merge before final fallback is considered.
            // Restore the incoming two-pass authority at this boundary when
            // the only extra ledger content is a repeated terminal revision.
            if let Some((authoritative_text, authoritative_segments)) =
                authoritative_final_candidate.as_ref()
            {
                if authoritative_final_supersedes_repeated_streaming_ledger(
                    authoritative_text,
                    &merged,
                ) {
                    log::warn!(
                        "[asr] authoritative final replaced inflated streaming merge final_chars={} merged_chars={}",
                        authoritative_text.chars().count(),
                        merged.chars().count()
                    );
                    merged = authoritative_text.clone();
                    segments = authoritative_segments.clone();
                }
            }
            // Hard multi-speaker isolation: never grow past the owner ceiling
            // while local identity is off the wake speaker.
            let (clamped_text, clamped_segments) =
                clamp_to_owner_isolation_ceiling(&state, merged, segments);
            merged = clamped_text;
            segments = clamped_segments;
            // Root contract: protocol final is a stream seal. Empty / weaker
            // finals (including Volcengine two_pass_empty and diarization
            // re-attribution stubs) never erase longer session-committed speech.
            // When isolation froze, the fallback deliberately excludes longer
            // optimistic room text.
            if has_final
                && (two_pass_empty_final
                    || final_speaker_candidate_empty
                    || spoken_content_len(&state.optimistic_preview_text)
                        > spoken_content_len(&merged)
                    || spoken_content_len(&state.best_transcript_text)
                        > spoken_content_len(&merged)
                    || (state.owner_isolation_frozen
                        && spoken_content_len(&state.owner_isolation_ceiling_text)
                            > spoken_content_len(&merged)))
            {
                if let Some((fallback_text, fallback_segments)) =
                    confirmed_owner_final_preview_fallback(&state, &merged)
                {
                    if authoritative_two_pass
                        && authoritative_final_supersedes_repeated_streaming_ledger(
                            &merged,
                            &fallback_text,
                        )
                    {
                        log::warn!(
                            "[asr] authoritative final rejected repeated streaming-ledger tail final_chars={} ledger_chars={}",
                            merged.chars().count(),
                            fallback_text.chars().count()
                        );
                    } else {
                        log::warn!(
                            "[asr] protocol final weaker than session ledger; committing {} accepted chars over {} retained chars (two_pass_empty={} final_empty={} isolation_frozen={})",
                            fallback_text.chars().count(),
                            merged.chars().count(),
                            two_pass_empty_final,
                            final_speaker_candidate_empty,
                            state.owner_isolation_frozen
                        );
                        merged = fallback_text;
                        segments = fallback_segments;
                    }
                }
            }
            let cleaned_streaming_tail = trim_repeated_short_streaming_tail(&merged);
            if cleaned_streaming_tail != merged {
                log::info!(
                    "[asr] trimmed repeated short streaming tail ({} -> {} chars)",
                    merged.chars().count(),
                    cleaned_streaming_tail.chars().count()
                );
                merged = cleaned_streaming_tail;
            }
            if has_final {
                let normalized = normalize_cjk_final_spacing_and_echoes(&merged);
                if normalized != merged {
                    log::info!(
                        "[asr] normalized CJK final spacing/echoes ({} -> {} chars)",
                        merged.chars().count(),
                        normalized.chars().count()
                    );
                    merged = normalized;
                }
                let trimmed = trim_repeated_short_final_tail(&merged);
                if trimmed != merged {
                    log::info!(
                        "[asr] trimmed repeated short final tail ({} -> {} chars)",
                        merged.chars().count(),
                        trimmed.chars().count()
                    );
                    merged = trimmed;
                }
            }
            let changed = !merged.is_empty() && state.last_partial_text != merged;
            if !merged.is_empty() {
                state.best_transcript_text = merged.clone();
                state.best_transcript_segments = segments;
                state.last_partial_text = merged.clone();
            }
            // Never let an incomplete authoritative intermediate (or an
            // empty/shorter protocol final) erase a longer owner preview that
            // was already accepted for the capsule UI.
            if !pending_unattributed_speech || has_final {
                let preview_len = spoken_content_len(&state.optimistic_preview_text);
                let merged_len = spoken_content_len(&merged);
                let preserve_owner_preview =
                    should_preserve_longer_owner_preview(&state, &merged, has_final);
                if !preserve_owner_preview && (!has_final || merged_len >= preview_len) {
                    state.optimistic_preview_text = merged.clone();
                    state.optimistic_preview_segments = state.best_transcript_segments.clone();
                    state.optimistic_untimed_window.clear();
                }
            }
            let preview_holds_endpoint = !has_final
                && changed
                && refresh_local_target_from_owner_preview_activity(&mut state);
            (merged, changed, preview_holds_endpoint)
        };

        // Owner ASR text is still growing while debounced identity remains
        // target: keep the 1s endpoint clock aligned with live speech so
        // mid-sentence embedding dips cannot freeze local_target_end and cut.
        if preview_holds_endpoint {
            let update = {
                let state = self.state.lock();
                target_speaker_update_from_state(&state, true, false)
            };
            log::info!(
                "[asr] owner authoritative preview growth refreshed local target endpoint clock local_target_end_ms={:?}",
                update.local_target_speech_end_ms
            );
            self.emit_target_speaker_update(update);
        }

        let preserve_longer_owner_preview = {
            let state = self.state.lock();
            should_preserve_longer_owner_preview(&state, &full_text, has_final)
        };
        if preserve_longer_owner_preview {
            log::info!(
                "[asr] suppressed non-final preview regression candidate_chars={} retained_chars={}",
                full_text.chars().count(),
                self.state.lock().last_emitted_preview_text.chars().count()
            );
        }

        if !has_final
            && authoritative_two_pass
            && partial_changed
            && !full_text.is_empty()
            && !preserve_longer_owner_preview
        {
            let elapsed_ms = self
                .state
                .lock()
                .start
                .map(|s| s.elapsed().as_millis() as u64)
                .unwrap_or(0);
            log::info!(
                "[asr] final supplemental partial chars={} elapsed_ms={}",
                full_text.chars().count(),
                elapsed_ms
            );
            self.emit_final_intermediate_transcript(FinalIntermediateTranscript {
                text: full_text.clone(),
                authoritative_two_pass,
            });
        }

        // 缓存最新的 transcript：服务端在 final 帧前断连时 fallback 用，
        // 同时把稳定预览推给胶囊。final 也要推一次，因为火山 two-pass
        // 经常在 final 才补齐长句前半段；这能让胶囊消失前先显示完整预览。
        let should_emit_authoritative_preview = (has_final
            || (!pending_unattributed_speech
                && self
                    .session_options
                    .endpoint
                    .emits_stream_preview_before_final()))
            && !full_text.is_empty()
            && !preserve_longer_owner_preview
            && {
                let mut state = self.state.lock();
                let changed = state.last_emitted_preview_text != full_text;
                if changed || has_final {
                    state.last_emitted_preview_text = full_text.clone();
                    state.partial_updates_seen += 1;
                }
                changed || has_final
            };
        if should_emit_authoritative_preview {
            let elapsed_ms = self
                .state
                .lock()
                .start
                .map(|s| s.elapsed().as_millis() as u64)
                .unwrap_or(0);
            log::info!(
                "[asr] {} partial update chars={} final={} elapsed_ms={}",
                self.session_options.endpoint.label(),
                full_text.chars().count(),
                has_final,
                elapsed_ms
            );
            self.emit_partial_transcript(&full_text);
            self.emit_streaming_event(if has_final {
                VolcengineStreamingEvent::Final(full_text.clone())
            } else {
                VolcengineStreamingEvent::Partial(full_text.clone())
            });
        }

        if has_final {
            let duration_ms = self
                .state
                .lock()
                .start
                .map(|s| s.elapsed().as_millis() as u64)
                .unwrap_or(0);
            let transcript = RawTranscript {
                text: full_text,
                duration_ms,
            };
            self.signal_success(transcript);
            self.state.lock().is_connected = false;
            *self.audio_tx.lock() = None;
            return false;
        }
        true
    }

    fn signal_success(&self, transcript: RawTranscript) {
        let tx = self.state.lock().final_tx.take();
        if let Some(tx) = tx {
            let _ = tx.send(Ok(transcript));
        }
    }

    fn signal_error(&self, err: VolcengineASRError) {
        self.emit_streaming_event(VolcengineStreamingEvent::Error(err.clone()));
        self.signal_error_silently(err);
    }

    fn signal_error_silently(&self, err: VolcengineASRError) {
        let tx = self.state.lock().final_tx.take();
        if let Some(tx) = tx {
            let _ = tx.send(Err(err));
        }
    }

    /// 服务端 close / 网络中断时调用：提交本轮会话账本里最长的已确认文本。
    fn fallback_to_partial_or_error(&self, err: VolcengineASRError) {
        let delivery_error = err.clone();
        let (partial, duration_ms) = {
            let st = self.state.lock();
            (
                session_committed_transcript(&st).map(|(text, _)| text),
                st.start
                    .map(|s| s.elapsed().as_millis() as u64)
                    .unwrap_or(0),
            )
        };
        if let Some(partial) = partial.filter(|text| !text.trim().is_empty()) {
            log::warn!(
                "[asr] {}; 使用会话账本兜底（{} 字）",
                err,
                partial.chars().count()
            );
            self.signal_success(RawTranscript {
                text: partial,
                duration_ms,
            });
        } else {
            self.signal_error(err);
        }
        self.mark_audio_delivery_failed(delivery_error);
        self.state.lock().is_connected = false;
        *self.audio_tx.lock() = None;
    }
}

impl AudioConsumer for VolcengineStreamingASR {
    fn consume_pcm_chunk(&self, pcm: &[u8]) {
        // Keep accepting capture after a live delivery failure. The coordinator
        // may still be receiving the final device packets, and the one-shot
        // recovery replay must contain the complete recording rather than only
        // the prefix that reached the failed socket.
        if !pcm.is_empty() && !self.state.lock().finishing {
            self.retained_pcm.lock().extend_from_slice(pcm);
        }
        #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
        if !pcm.is_empty() {
            if let Some(stream) = self.target_speaker_stream.lock().as_ref().cloned() {
                stream.consume_pcm_chunk(pcm);
            }
        }
        // 单 worker 串行 send 模式：在 state 锁内 drain 并分配 seq（seq 单调），
        // 然后把 (seq, chunk) push 进 mpsc。worker 端按入队顺序 send，
        // 哪怕跨多个 consume 调用、多个 spawn 也不会再有 writer 锁竞争。
        let chunks: Vec<(i32, Vec<u8>)> = {
            let mut st = self.state.lock();
            if !st.is_connected || st.finishing {
                return;
            }
            st.pending_audio.extend_from_slice(pcm);

            let mut out: Vec<(i32, Vec<u8>)> = Vec::new();
            while st.pending_audio.len() >= TARGET_AUDIO_CHUNK_BYTES {
                let chunk: Vec<u8> = st.pending_audio.drain(..TARGET_AUDIO_CHUNK_BYTES).collect();
                let seq = st.next_sequence;
                st.next_sequence += 1;
                st.bytes_sent += chunk.len();
                st.frames_sent += 1;
                out.push((seq, chunk));
            }
            out
        };

        if chunks.is_empty() {
            return;
        }
        let Some(tx) = self.audio_tx.lock().as_ref().cloned() else {
            return;
        };

        for entry in chunks {
            // pending_sends 必须在 tx.send 之前 +1：否则 worker 可能先 recv + 发送 +
            // 减 1，把 usize 计数器 underflow。
            let pending_frames = self.pending_sends.fetch_add(1, Ordering::SeqCst) + 1;
            update_pending_send_high_water(&self.pending_sends_high_water, pending_frames);
            if tx.send(entry).is_err() {
                // worker 已退出（cancel / 错误路径里 audio_tx 被 take）。
                // 撤销刚才的 +1，避免 send_last_frame 的 wait 永远等不到 0。
                if self.pending_sends.fetch_sub(1, Ordering::SeqCst) == 1 {
                    self.send_done.notify_waiters();
                }
                log::warn!("[asr] audio queue closed; dropping subsequent frames");
                return;
            }
        }
    }
}

fn update_pending_send_high_water(high_water: &AtomicUsize, pending_frames: usize) {
    let mut observed = high_water.load(Ordering::Relaxed);
    while pending_frames > observed {
        match high_water.compare_exchange_weak(
            observed,
            pending_frames,
            Ordering::SeqCst,
            Ordering::Relaxed,
        ) {
            Ok(_) => break,
            Err(current) => observed = current,
        }
    }
}

fn final_audio_drain_budget(pending_frames: usize) -> Duration {
    let backlog_ms = u64::try_from(pending_frames)
        .unwrap_or(u64::MAX)
        .saturating_mul(AUDIO_FRAME_DURATION_MS);
    std::cmp::min(
        FINAL_AUDIO_DRAIN_MAX_BUDGET,
        std::cmp::max(
            FINAL_AUDIO_DRAIN_MIN_BUDGET,
            Duration::from_millis(backlog_ms),
        ),
    )
}

async fn send_binary(writer: &SharedWriter, data: Vec<u8>) -> Result<(), VolcengineASRError> {
    send_binary_with_timeout(writer, data, WEBSOCKET_SEND_TIMEOUT).await
}

async fn send_binary_with_timeout(
    writer: &SharedWriter,
    data: Vec<u8>,
    timeout: Duration,
) -> Result<(), VolcengineASRError> {
    let mut guard = tokio::time::timeout(timeout, writer.lock())
        .await
        .map_err(|_| {
            VolcengineASRError::ConnectionFailed(format!(
                "websocket writer lock timed out after {} ms",
                timeout.as_millis()
            ))
        })?;
    let Some(sink) = guard.as_mut() else {
        return Err(VolcengineASRError::ConnectionFailed(
            "websocket not open".into(),
        ));
    };
    tokio::time::timeout(timeout, sink.send(Message::Binary(data)))
        .await
        .map_err(|_| {
            VolcengineASRError::ConnectionFailed(format!(
                "websocket send timed out after {} ms",
                timeout.as_millis()
            ))
        })?
        .map_err(|e| VolcengineASRError::ConnectionFailed(e.to_string()))
}

fn hex_prefix(data: &[u8], n: usize) -> String {
    data.iter()
        .take(n)
        .map(|b| format!("{:02x}", b))
        .collect::<Vec<_>>()
        .join("")
}

/// 把 tokio-tungstenite 的 connect 错误分类：握手收到 HTTP 401 / 403 → `AuthRejected`
/// （凭据被拒，要 user 检查 App ID / Access Token / 账号资源开通状态）；其它 → 通用
/// `ConnectionFailed`（DNS / TLS / 网络层）。让 capsule 文案能跟泛泛 HTTP error 区分。
fn classify_connect_error(err: tokio_tungstenite::tungstenite::Error) -> VolcengineASRError {
    use tokio_tungstenite::tungstenite::Error as WsError;
    if let WsError::Http(resp) = &err {
        let status = resp.status().as_u16();
        if status == 401 || status == 403 {
            return VolcengineASRError::AuthRejected(status);
        }
    }
    VolcengineASRError::ConnectionFailed(err.to_string())
}

fn hotword_context(entries: &[DictionaryHotword]) -> Option<String> {
    let mut seen: Vec<String> = Vec::new();
    for entry in entries {
        if !entry.enabled {
            continue;
        }
        let trimmed = entry.phrase.trim();
        if trimmed.is_empty() {
            continue;
        }
        if seen.iter().any(|w| w.eq_ignore_ascii_case(trimmed)) {
            continue;
        }
        seen.push(trimmed.to_string());
        if seen.len() >= HOTWORD_CAP {
            break;
        }
    }
    if seen.is_empty() {
        return None;
    }
    let words: Vec<Value> = seen.into_iter().map(|w| json!({ "word": w })).collect();
    let payload = json!({ "hotwords": words });
    serde_json::to_string(&payload).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
    #[test]
    fn target_speaker_final_wait_is_reserved_for_real_interference() {
        assert!(!target_speaker_final_required(false, false, false));
        assert!(target_speaker_final_required(true, false, false));
        assert!(target_speaker_final_required(false, true, false));
        assert!(target_speaker_final_required(false, false, true));
    }

    #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
    #[test]
    fn sustained_tail_score_collapse_triggers_late_overlap_filter_only() {
        fn timeline(scores: &[f32]) -> Vec<LocalSpeakerEvidence> {
            scores
                .iter()
                .enumerate()
                .map(|(index, score)| LocalSpeakerEvidence {
                    audio_end_ms: 1_000 + index as u64 * 400,
                    classification: if *score >= 0.55 {
                        crate::speaker_verification::SessionSpeakerClassification::Target {
                            score: *score,
                        }
                    } else {
                        crate::speaker_verification::SessionSpeakerClassification::Uncertain {
                            score: *score,
                        }
                    },
                    stable_target: *score >= 0.55,
                })
                .collect()
        }

        let clean = timeline(&[
            0.559, 0.524, 0.648, 0.737, 0.696, 0.706, 0.718, 0.786, 0.819, 0.890, 0.909,
        ]);
        let late_overlap = timeline(&[
            0.559, 0.524, 0.648, 0.737, 0.696, 0.706, 0.709, 0.626, 0.491, 0.557, 0.556,
        ]);
        assert!(!degraded_owner_tail_suggests_interference(&clean));
        assert!(degraded_owner_tail_suggests_interference(&late_overlap));
    }

    #[test]
    fn pending_send_high_water_retains_the_peak() {
        let high_water = AtomicUsize::new(0);

        update_pending_send_high_water(&high_water, 1);
        update_pending_send_high_water(&high_water, 4);
        update_pending_send_high_water(&high_water, 2);

        assert_eq!(high_water.load(Ordering::SeqCst), 4);
    }

    #[test]
    fn hotword_context_dedupes_case_insensitively_and_caps() {
        let mut entries = vec![
            DictionaryHotword {
                phrase: "Foo".into(),
                enabled: true,
            },
            DictionaryHotword {
                phrase: "foo".into(),
                enabled: true,
            },
            DictionaryHotword {
                phrase: "  ".into(),
                enabled: true,
            },
            DictionaryHotword {
                phrase: "Bar".into(),
                enabled: false,
            },
            DictionaryHotword {
                phrase: "Baz".into(),
                enabled: true,
            },
        ];
        for i in 0..200 {
            entries.push(DictionaryHotword {
                phrase: format!("w{}", i),
                enabled: true,
            });
        }
        let ctx = hotword_context(&entries).expect("should produce JSON");
        assert!(ctx.contains("\"hotwords\""));
        assert!(ctx.contains("Foo"));
        assert!(ctx.contains("Baz"));
        assert!(!ctx.contains("Bar"));
        let count = ctx.matches("\"word\"").count();
        assert!(count <= HOTWORD_CAP);
    }

    #[test]
    fn hotword_context_returns_none_when_all_disabled() {
        let entries = vec![DictionaryHotword {
            phrase: "Foo".into(),
            enabled: false,
        }];
        assert!(hotword_context(&entries).is_none());
    }

    #[test]
    fn default_resource_id_uses_seed_asr_2_hourly_quota() {
        assert_eq!(
            VolcengineCredentials::default_resource_id(),
            "volc.seedasr.sauc.duration"
        );
    }

    #[test]
    fn cancel_does_not_emit_a_platform_provider_error() {
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        let events = Arc::new(ParkingMutex::new(Vec::new()));
        let events_for_callback = Arc::clone(&events);
        asr.set_streaming_event_callback(Some(Arc::new(move |event| {
            events_for_callback.lock().push(event);
        })));

        asr.cancel();

        assert!(events.lock().is_empty());
    }

    #[test]
    fn provider_response_metadata_excludes_transcript_text() {
        let json = json!({
            "audio_info": { "duration": 1_600 },
            "result": {
                "text": "private transcript",
                "utterances": [{
                    "additions": { "source": "two_pass", "speaker_id": "7" },
                    "definite": true,
                    "start_time": 120,
                    "end_time": 1_480,
                    "text": "private transcript"
                }]
            }
        });
        let metadata = provider_response_metadata(&json, &json["result"], false, true);

        assert_eq!(metadata["audio_duration_ms"], 1_600);
        assert_eq!(metadata["sources"], json!(["two_pass"]));
        assert_eq!(metadata["utterance_count"], 1);
        assert_eq!(metadata["has_final_frame"], false);
        assert_eq!(metadata["authoritative_two_pass"], true);
        assert_eq!(metadata["provider_result_chars"], 18);
        assert_eq!(metadata["result_chars"], 18);
        assert_eq!(metadata["final_speaker_timeline"], Value::Null);
        assert!(!metadata.to_string().contains("private transcript"));

        let final_metadata = provider_response_metadata(&json, &json["result"], true, true);
        assert_eq!(
            final_metadata["final_speaker_timeline"][0]["speaker_id"],
            "7"
        );
        assert_eq!(final_metadata["final_speaker_timeline"][0]["start_ms"], 120);
        assert_eq!(final_metadata["final_speaker_timeline"][0]["end_ms"], 1_480);
        assert_eq!(final_metadata["final_speaker_timeline"][0]["chars"], 18);
        assert!(!final_metadata.to_string().contains("private transcript"));
    }

    #[test]
    fn optimized_bidirectional_diagnostic_mode_enables_official_second_pass() {
        let asr = VolcengineStreamingASR::new_with_session_options(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
            VolcengineSessionOptions {
                endpoint: VolcengineSessionEndpoint::OptimizedBidirectional,
                enable_nonstream: true,
                result_type: VolcengineResultType::Full,
                end_window_size_ms: Some(SECOND_PASS_END_WINDOW_MS),
                force_to_speech_time_ms: Some(SECOND_PASS_FORCE_TO_SPEECH_MS),
            },
        );

        assert_eq!(
            asr.session_options.endpoint.endpoint(),
            FINAL_TRANSCRIPT_ENDPOINT
        );
        let payload = asr.build_first_frame_payload("test-connect-id");
        let request = &payload["request"];

        assert_eq!(request["enable_nonstream"], true);
        assert_eq!(request["enable_accelerate_text"], true);
        assert_eq!(request["result_type"], "full");
        assert_eq!(request["end_window_size"], SECOND_PASS_END_WINDOW_MS);
        assert_eq!(
            request["force_to_speech_time"],
            SECOND_PASS_FORCE_TO_SPEECH_MS
        );
    }

    #[test]
    fn default_session_uses_one_optimized_bidirectional_engine_for_preview_and_final_result() {
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );

        assert_eq!(
            asr.session_options.endpoint.endpoint(),
            FINAL_TRANSCRIPT_ENDPOINT
        );
        let payload = asr.build_first_frame_payload("test-connect-id");
        let request = &payload["request"];
        assert_eq!(request["enable_punc"], true);
        assert_eq!(request["result_type"], "full");
        assert_eq!(request["show_utterances"], true);
        assert_eq!(request["enable_speaker_info"], true);
        assert_eq!(request["enable_nonstream"], true);
        assert_eq!(request["enable_accelerate_text"], true);
        assert_eq!(request["end_window_size"], SECOND_PASS_END_WINDOW_MS);
        assert_eq!(
            request["force_to_speech_time"],
            SECOND_PASS_FORCE_TO_SPEECH_MS
        );
        assert!(VolcengineSessionEndpoint::Bidirectional.emits_stream_preview_before_final());
        assert!(
            VolcengineSessionEndpoint::OptimizedBidirectional.emits_stream_preview_before_final()
        );
    }

    #[test]
    fn optimized_bidirectional_emits_speakerless_early_text_without_promoting_it_to_final_state() {
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        let previews = Arc::new(ParkingMutex::new(Vec::new()));
        let previews_for_callback = Arc::clone(&previews);
        asr.set_partial_transcript_callback(Some(Arc::new(move |text| {
            previews_for_callback.lock().push(text);
        })));

        let payload = serde_json::to_vec(&json!({
            "audio_info": { "duration": 900 },
            "result": {
                "text": "请在今天",
                "utterances": [{
                    "additions": { "source": "stream" },
                    "definite": false,
                    "end_time": 899,
                    "text": "请在今天",
                    "words": [{ "start_time": 360, "end_time": 899, "text": "请在今天" }]
                }]
            }
        }))
        .expect("stream response serializes");
        let frame = frame::build(
            MessageType::FullServerResponse,
            Flags::None,
            Serialization::Json,
            &payload,
            None,
        );

        assert!(asr.handle_frame(&frame));
        let growing_payload = serde_json::to_vec(&json!({
            "audio_info": { "duration": 1200 },
            "result": {
                "text": "请在今天下午",
                "utterances": [{
                    "additions": { "source": "stream" },
                    "definite": false,
                    "start_time": 360,
                    "end_time": 1199,
                    "text": "请在今天下午",
                    "words": [{ "start_time": 360, "end_time": 1199, "text": "请在今天下午" }]
                }]
            }
        }))
        .expect("growing stream response serializes");
        let growing_frame = frame::build(
            MessageType::FullServerResponse,
            Flags::None,
            Serialization::Json,
            &growing_payload,
            None,
        );
        assert!(asr.handle_frame(&growing_frame));
        assert_eq!(
            &*previews.lock(),
            &["请在今天".to_string(), "请在今天下午".to_string()]
        );
        let state = asr.state.lock();
        assert_eq!(state.partial_updates_seen, 2);
        assert_eq!(state.optimistic_preview_text, "请在今天下午");
        assert!(state.best_transcript_text.is_empty());
        assert!(state.last_partial_text.is_empty());
    }

    #[test]
    fn protocol_final_preserves_continuously_confirmed_owner_preview_when_boundaries_drop_it() {
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        asr.note_local_speaker_tracking_started("开始录音");
        for audio_end_ms in [3_600, 4_000, 4_400, 4_800] {
            asr.note_local_speaker_classification(
                audio_end_ms,
                crate::speaker_verification::SessionSpeakerClassification::Target { score: 0.6 },
            );
        }
        asr.state.lock().target_speaker_id = Some("0".into());

        let preview_payload = serde_json::to_vec(&json!({
            "audio_info": { "duration": 5_300 },
            "result": {
                "text": "现在好像应该没有什么别的问题了吧？我看了",
                "utterances": [{
                    "additions": { "source": "stream" },
                    "definite": false,
                    "start_time": 1_300,
                    "end_time": 4_900,
                    "text": "现在好像应该没有什么别的问题了吧？我看了"
                }]
            }
        }))
        .expect("preview response serializes");
        let preview_frame = frame::build(
            MessageType::FullServerResponse,
            Flags::None,
            Serialization::Json,
            &preview_payload,
            None,
        );
        assert!(asr.handle_frame(&preview_frame));
        assert_eq!(
            asr.state.lock().optimistic_preview_text,
            "现在好像应该没有什么别的问题了吧？我看了"
        );
        // Target-confirmed stream text is session speech ledger, not UI-only.
        assert_eq!(
            asr.state.lock().best_transcript_text,
            "现在好像应该没有什么别的问题了吧？我看了"
        );

        // A previously stabilized wake-only prefix is shorter than the ledger
        // body and must not win over confirmed owner speech at final time.
        asr.state.lock().best_transcript_text = "开始录音".into();
        // Keep optimistic preview as the longer committed body.

        let (tx, mut rx) = oneshot::channel();
        asr.state.lock().final_tx = Some(tx);
        let final_payload = serde_json::to_vec(&json!({
            "audio_info": { "duration": 6_000 },
            "result": {
                "text": "能录音 现在好像应该没有什么别的问题了吧？我看。",
                "utterances": [
                    {
                        "additions": { "speaker_id": "0", "source": "two_pass" },
                        "definite": true,
                        "start_time": 0,
                        "end_time": 1_232,
                        "text": "能录音"
                    },
                    {
                        "additions": { "speaker_id": "0", "source": "two_pass" },
                        "definite": true,
                        "start_time": 5_000,
                        "end_time": 5_492,
                        "text": "现在好像应该没有什么别的问题了吧？我看。"
                    }
                ]
            }
        }))
        .expect("final response serializes");
        let final_frame = frame::build(
            MessageType::FullServerResponse,
            Flags::LastPacket,
            Serialization::Json,
            &final_payload,
            None,
        );

        assert!(!asr.handle_frame(&final_frame));
        let transcript = rx
            .try_recv()
            .expect("protocol final should resolve the transcript")
            .expect("confirmed owner preview should remain successful");
        assert_eq!(transcript.text, "现在好像应该没有什么别的问题了吧？我看了");
    }

    #[test]
    fn final_preview_fallback_keeps_confirmed_owner_body_after_non_target_tail() {
        let mut manual = SyncState::default();
        // Empty ledger → nothing to commit.
        assert!(confirmed_owner_final_preview_fallback(&manual, "").is_none());

        manual.optimistic_preview_text = "这是应该保留的本人正文".into();
        // Production commits a Target-confirmed owner preview into the ledger
        // via `commit_session_transcript_if_stronger`; the isolation branch of
        // `session_committed_transcript` deliberately never reads optimistic
        // directly (it may have absorbed room speech before the gate closed).
        manual.best_transcript_text = "这是应该保留的本人正文".into();
        manual.local_speaker_tracking_enabled = true;
        manual.local_target_confirmed = true;
        // Final often arrives after other people / ambient NonTarget frames.
        // That must not discard the already-confirmed owner preview.
        manual.local_speaker_stable_target = false;
        manual.local_non_target_speech_end_ms = Some(12_200);
        manual.local_speaker_classification = Some(
            crate::speaker_verification::SessionSpeakerClassification::NonTarget { score: 0.22 },
        );
        assert_eq!(
            confirmed_owner_final_preview_fallback(&manual, "").map(|(text, _)| text),
            Some("这是应该保留的本人正文".into())
        );
        assert_eq!(
            confirmed_owner_final_preview_fallback(&manual, "开始录音").map(|(text, _)| text),
            Some("这是应该保留的本人正文".into())
        );
        // Longer retained finals still win over the cached preview.
        assert!(confirmed_owner_final_preview_fallback(
            &manual,
            "这是一个明显更加完整而且已经稳定的正文"
        )
        .is_none());
    }

    #[test]
    fn authoritative_final_frame_beats_inflated_repeated_session_ledger() {
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        let authoritative =
            "开始录音你继续把那个脸和录音波这个东西给做完，然后告诉我验收一下，然后我现在可以有时间验收了。";
        let inflated =
            "开始录音你继续把那个脸和读音波这个东西给做完，然后告我验收一下，然后我现在可以有时间验收了那个脸和读音波这个东西给做完，然后告我验收一下";
        {
            let mut state = asr.state.lock();
            state.best_transcript_text = inflated.into();
            state.last_partial_text = inflated.into();
            state.optimistic_preview_text = inflated.into();
            state.target_speaker_id = Some("0".into());
        }

        let (tx, mut rx) = oneshot::channel();
        asr.state.lock().final_tx = Some(tx);
        let payload = serde_json::to_vec(&json!({
            "audio_info": { "duration": 10_960 },
            "result": {
                "text": authoritative,
                "utterances": [{
                    "additions": { "source": "two_pass", "speaker_id": "0" },
                    "definite": true,
                    "start_time": 0,
                    "end_time": 7_622,
                    "text": authoritative
                }]
            }
        }))
        .expect("incident final serializes");
        let final_frame = frame::build(
            MessageType::FullServerResponse,
            Flags::LastPacket,
            Serialization::Json,
            &payload,
            None,
        );

        assert!(!asr.handle_frame(&final_frame));
        let transcript = rx
            .try_recv()
            .expect("final should resolve")
            .expect("authoritative final should succeed");
        assert_eq!(transcript.text, authoritative);
    }

    #[test]
    fn two_pass_empty_final_seals_stream_without_erasing_session_speech() {
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        asr.note_local_speaker_tracking_started("开始录音");
        for audio_end_ms in [2_000, 2_400, 2_800, 3_200] {
            asr.note_local_speaker_classification(
                audio_end_ms,
                crate::speaker_verification::SessionSpeakerClassification::Target { score: 0.7 },
            );
        }

        let preview_payload = serde_json::to_vec(&json!({
            "audio_info": { "duration": 4_000 },
            "result": {
                "text": "你帮我看一下这个产品还有什么可以改善的路径",
                "utterances": [{
                    "additions": { "source": "stream" },
                    "definite": false,
                    "start_time": 800,
                    "end_time": 3_800,
                    "text": "你帮我看一下这个产品还有什么可以改善的路径"
                }]
            }
        }))
        .expect("preview response serializes");
        let preview_frame = frame::build(
            MessageType::FullServerResponse,
            Flags::None,
            Serialization::Json,
            &preview_payload,
            None,
        );
        assert!(asr.handle_frame(&preview_frame));
        assert_eq!(
            asr.state.lock().best_transcript_text,
            "你帮我看一下这个产品还有什么可以改善的路径"
        );

        // Live failure shape from Volcengine: final frame with only
        // two_pass_empty=true and no result text.
        let (tx, mut rx) = oneshot::channel();
        asr.state.lock().final_tx = Some(tx);
        let final_payload = serde_json::to_vec(&json!({
            "audio_info": { "duration": 15_215 },
            "result": {
                "additions": { "log_id": "test-two-pass-empty" },
                "utterances": [{
                    "additions": {
                        "invoke_type": "hard_vad",
                        "source": "two_pass",
                        "two_pass_empty": "true"
                    },
                    "definite": true,
                    "end_time": 14_032
                }]
            }
        }))
        .expect("two_pass_empty final serializes");
        let final_frame = frame::build(
            MessageType::FullServerResponse,
            Flags::LastPacket,
            Serialization::Json,
            &final_payload,
            None,
        );

        assert!(!asr.handle_frame(&final_frame));
        let transcript = rx
            .try_recv()
            .expect("two_pass_empty final must still resolve")
            .expect("session speech must survive two_pass_empty seal");
        assert_eq!(
            transcript.text,
            "你帮我看一下这个产品还有什么可以改善的路径"
        );
        assert!(result_marks_two_pass_empty(&json!({
            "utterances": [{
                "additions": { "source": "two_pass", "two_pass_empty": "true" },
                "definite": true,
                "end_time": 1
            }]
        })));
    }

    #[test]
    fn protocol_final_preserves_owner_preview_after_non_target_tail_empties_boundaries() {
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        asr.note_local_speaker_tracking_started("开始录音");
        for audio_end_ms in [3_600, 4_000, 4_400, 4_800, 5_200] {
            asr.note_local_speaker_classification(
                audio_end_ms,
                crate::speaker_verification::SessionSpeakerClassification::Target { score: 0.62 },
            );
        }
        asr.state.lock().target_speaker_id = Some("0".into());

        let preview_payload = serde_json::to_vec(&json!({
            "audio_info": { "duration": 5_800 },
            "result": {
                "text": "你帮我看一下这个产品还有什么可以改善的路径",
                "utterances": [{
                    "additions": { "source": "stream" },
                    "definite": false,
                    "start_time": 1_000,
                    "end_time": 5_500,
                    "text": "你帮我看一下这个产品还有什么可以改善的路径"
                }]
            }
        }))
        .expect("preview response serializes");
        let preview_frame = frame::build(
            MessageType::FullServerResponse,
            Flags::None,
            Serialization::Json,
            &preview_payload,
            None,
        );
        assert!(asr.handle_frame(&preview_frame));
        assert_eq!(
            asr.state.lock().optimistic_preview_text,
            "你帮我看一下这个产品还有什么可以改善的路径"
        );

        // Match the live failure: owner body finished, then a low-score local
        // tail arrived before protocol final. It may hide provisional room text,
        // but it must not freeze/delete the accepted owner ledger.
        for audio_end_ms in [11_500, 11_900, 12_300] {
            asr.note_local_speaker_classification(
                audio_end_ms,
                crate::speaker_verification::SessionSpeakerClassification::NonTarget {
                    score: 0.28,
                },
            );
        }
        assert!(asr.state.lock().local_speaker_stable_target);
        assert!(!asr.state.lock().owner_isolation_frozen);

        let (tx, mut rx) = oneshot::channel();
        asr.state.lock().final_tx = Some(tx);
        // Provider final only keeps the other speaker → filtered empty.
        let final_payload = serde_json::to_vec(&json!({
            "audio_info": { "duration": 14_000 },
            "result": {
                "text": "旁边别人插了一句",
                "utterances": [{
                    "additions": { "speaker_id": "1", "source": "two_pass" },
                    "definite": true,
                    "start_time": 11_200,
                    "end_time": 13_400,
                    "text": "旁边别人插了一句"
                }]
            }
        }))
        .expect("final response serializes");
        let final_frame = frame::build(
            MessageType::FullServerResponse,
            Flags::LastPacket,
            Serialization::Json,
            &final_payload,
            None,
        );

        assert!(!asr.handle_frame(&final_frame));
        let transcript = rx
            .try_recv()
            .expect("protocol final should resolve the transcript")
            .expect("confirmed owner preview must survive non-target final reattribution");
        assert_eq!(
            transcript.text,
            "你帮我看一下这个产品还有什么可以改善的路径"
        );
    }

    #[test]
    fn speaker_filter_locks_first_speaker_and_excludes_other_people() {
        let result = json!({
            "text": "目标说话旁人插话目标继续",
            "utterances": [
                {
                    "additions": { "speaker": "1", "source": "stream" },
                    "definite": true,
                    "start_time": 100,
                    "end_time": 700,
                    "text": "目标说话"
                },
                {
                    "additions": { "speaker": "2", "source": "stream" },
                    "definite": true,
                    "start_time": 800,
                    "end_time": 1300,
                    "text": "旁人插话"
                },
                {
                    "additions": { "speaker": "1", "source": "stream" },
                    "definite": true,
                    "start_time": 1400,
                    "end_time": 1900,
                    "text": "目标继续"
                }
            ]
        });
        let mut target = None;

        let filtered = filter_result_to_target_speaker(&result, &mut target);

        assert_eq!(target.as_deref(), Some("1"));
        assert!(filtered.speaker_info_present);
        assert_eq!(filtered.result["text"], "目标说话目标继续");
        assert_eq!(filtered.result["utterances"].as_array().unwrap().len(), 2);
        assert_eq!(filtered.target_speech_end_ms, Some(1900));
        assert_eq!(filtered.stable_attributed_speech_end_ms, Some(1900));
        assert!(filtered.pending_unattributed_text.is_empty());
    }

    #[test]
    fn growing_unattributed_tail_blocks_endpoint_without_leaking_into_output() {
        let result = json!({
            "text": "这个东西现在能不能弄？然后帮我看一下",
            "utterances": [{
                "additions": { "speaker_id": "0", "source": "two_pass" },
                "definite": true,
                "start_time": 312,
                "end_time": 2542,
                "text": "这个东西现在能不能弄？"
            }]
        });
        let mut target = None;

        let filtered = filter_result_to_target_speaker(&result, &mut target);

        assert_eq!(target.as_deref(), Some("0"));
        assert_eq!(filtered.result["text"], "这个东西现在能不能弄？");
        assert_eq!(
            filtered.pending_unattributed_text,
            "这个东西现在能不能弄？然后帮我看一下"
        );
        assert_eq!(
            filtered.optimistic_result["text"],
            "这个东西现在能不能弄？然后帮我看一下"
        );
    }

    #[test]
    fn final_raw_tail_after_first_utterance_is_kept_in_optimistic_text() {
        // Installed 4ff44fc3: cloud result.text had the full sentence while the
        // only stable utterance was "开始录音，那你". Final must be able to pick
        // the longer optimistic text (handle_frame prefers it when owner-safe).
        let result = json!({
            "text": "开始录音，那你 哦，搞个目标，修一下这个。这个够了，这个目标。",
            "utterances": [{
                "additions": { "speaker_id": "0", "source": "two_pass" },
                "definite": true,
                "start_time": 122,
                "end_time": 2132,
                "text": "开始录音，那你"
            }]
        });
        let mut target = None;
        let filtered = filter_result_to_target_speaker(&result, &mut target);
        assert_eq!(target.as_deref(), Some("0"));
        assert_eq!(filtered.result["text"], "开始录音，那你");
        let optimistic = filtered
            .optimistic_result
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            spoken_content_len(optimistic) > spoken_content_len("开始录音，那你"),
            "optimistic must keep the raw tail after the first stable utterance, got {optimistic:?}"
        );
        assert!(
            optimistic.contains("搞个目标"),
            "optimistic should include the later body, got {optimistic:?}"
        );
    }

    #[test]
    fn provisional_speaker_split_blocks_endpoint_until_provider_stabilizes() {
        let result = json!({
            "text": "根因已修现在新版本正在运行之前是云端只给一段",
            "utterances": [
                {
                    "additions": { "speaker_id": "0", "source": "two_pass" },
                    "definite": true,
                    "start_time": 542,
                    "end_time": 5572,
                    "text": "根因已修现在新版本正在运行之前是云端只给",
                    "words": [{ "start_time": 5000, "end_time": 5200, "text": "给" }]
                },
                {
                    "additions": { "speaker_id": "1", "source": "stream" },
                    "definite": false,
                    "start_time": 5700,
                    "end_time": 6900,
                    "text": "一段"
                }
            ]
        });
        let mut target = Some("0".to_string());

        let filtered = filter_result_to_target_speaker(&result, &mut target);

        assert_eq!(filtered.target_speech_end_ms, Some(5572));
        assert_eq!(
            filtered.result["text"],
            "根因已修现在新版本正在运行之前是云端只给"
        );
        assert!(!filtered.pending_unattributed_text.is_empty());
        assert_eq!(
            filtered.optimistic_result["text"],
            "根因已修现在新版本正在运行之前是云端只给一段"
        );
        assert!(filtered.speaker_info_present);
    }

    #[test]
    fn local_non_target_speech_never_enters_optimistic_preview() {
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        let previews = Arc::new(ParkingMutex::new(Vec::new()));
        let previews_for_callback = Arc::clone(&previews);
        asr.set_partial_transcript_callback(Some(Arc::new(move |text| {
            previews_for_callback.lock().push(text);
        })));
        asr.note_local_speaker_tracking_started("开始录音");
        asr.note_local_speaker_classification(
            700,
            crate::speaker_verification::SessionSpeakerClassification::Target { score: 0.6 },
        );
        asr.note_local_speaker_classification(
            1700,
            crate::speaker_verification::SessionSpeakerClassification::NonTarget { score: 0.2 },
        );

        let provisional_payload = serde_json::to_vec(&json!({
            "audio_info": { "duration": 1800 },
            "result": {
                "text": "目标正文临时旁人",
                "utterances": [
                    {
                        "additions": { "speaker": "0", "source": "two_pass" },
                        "definite": true,
                        "start_time": 100,
                        "end_time": 900,
                        "text": "目标正文"
                    },
                    {
                        "additions": { "speaker": "1", "source": "stream" },
                        "definite": false,
                        "start_time": 1000,
                        "end_time": 1700,
                        "text": "临时旁人"
                    }
                ]
            }
        }))
        .expect("provisional response serializes");
        let provisional_frame = frame::build(
            MessageType::FullServerResponse,
            Flags::None,
            Serialization::Json,
            &provisional_payload,
            None,
        );
        assert!(asr.handle_frame(&provisional_frame));
        assert!(previews.lock().is_empty());
        assert_eq!(asr.state.lock().best_transcript_text, "目标正文");

        let stable_payload = serde_json::to_vec(&json!({
            "audio_info": { "duration": 2100 },
            "result": {
                "text": "目标正文临时旁人",
                "utterances": [
                    {
                        "additions": { "speaker": "0", "source": "two_pass" },
                        "definite": true,
                        "start_time": 100,
                        "end_time": 900,
                        "text": "目标正文"
                    },
                    {
                        "additions": { "speaker": "1", "source": "two_pass" },
                        "definite": true,
                        "start_time": 1000,
                        "end_time": 1700,
                        "text": "临时旁人"
                    }
                ]
            }
        }))
        .expect("stable response serializes");
        let stable_frame = frame::build(
            MessageType::FullServerResponse,
            Flags::None,
            Serialization::Json,
            &stable_payload,
            None,
        );
        assert!(asr.handle_frame(&stable_frame));
        assert_eq!(&*previews.lock(), &["目标正文".to_string()]);
        let state = asr.state.lock();
        assert_eq!(state.best_transcript_text, "目标正文");
        assert_eq!(state.last_partial_text, "目标正文");
    }

    #[test]
    fn optimistic_preview_gate_allows_ephemeral_uncertain_without_extending_endpoint() {
        let mut state = SyncState::default();
        assert!(local_speaker_allows_optimistic_preview(&state));
        state.local_speaker_tracking_enabled = true;
        state.local_speaker_stable_target = true;
        state.local_speaker_classification =
            Some(crate::speaker_verification::SessionSpeakerClassification::Target { score: 0.6 });
        assert!(local_speaker_allows_optimistic_preview(&state));
        state.local_speaker_classification = Some(
            crate::speaker_verification::SessionSpeakerClassification::Uncertain { score: 0.38 },
        );
        assert!(local_speaker_allows_optimistic_preview(&state));
        assert!(!local_speaker_allows_owner_endpoint_refresh(&state));
        state.local_speaker_classification = Some(
            crate::speaker_verification::SessionSpeakerClassification::NonTarget { score: 0.2 },
        );
        assert!(!local_speaker_allows_optimistic_preview(&state));
    }

    #[test]
    fn stream_reset_preserves_the_pending_verified_wake_anchor() {
        let configured = SyncState {
            local_speaker_tracking_enabled: true,
            local_wake_owner_verified: true,
            local_speaker_profile_adaptive: false,
            wake_speaker_phrase: Some("开始录音".into()),
            ..SyncState::default()
        };
        let anchor = PendingSessionSpeakerAnchor::capture(&configured);
        let mut reset = SyncState::default();

        anchor.restore_after_stream_reset(&mut reset);

        assert!(reset.local_speaker_tracking_enabled);
        assert!(reset.local_wake_owner_verified);
        assert!(!reset.local_speaker_profile_adaptive);
        assert!(reset.local_speaker_stable_target);
        assert_eq!(reset.wake_speaker_phrase.as_deref(), Some("开始录音"));

        let disabled = SyncState {
            local_speaker_tracking_enabled: false,
            local_wake_owner_verified: true,
            local_speaker_profile_adaptive: true,
            ..SyncState::default()
        };
        let mut reset_disabled = SyncState::default();
        PendingSessionSpeakerAnchor::capture(&disabled)
            .restore_after_stream_reset(&mut reset_disabled);
        assert!(!reset_disabled.local_wake_owner_verified);
        assert!(!reset_disabled.local_speaker_profile_adaptive);
    }

    #[test]
    fn diarization_pending_visual_preview_advances_without_touching_final_or_endpoint_ledgers() {
        let mut state = SyncState {
            local_speaker_tracking_enabled: true,
            local_wake_owner_verified: true,
            local_speaker_stable_target: true,
            local_speaker_classification: Some(
                crate::speaker_verification::SessionSpeakerClassification::Uncertain {
                    score: 0.33,
                },
            ),
            last_emitted_preview_text: "开始录音。这是已确认的主讲人前半句。".into(),
            best_transcript_text: "开始录音。这是已确认的主讲人前半句。".into(),
            local_target_speech_end_ms: Some(6_800),
            ..SyncState::default()
        };
        let provider = json!({
            "text": "开始录音。这是已确认的主讲人前半句，后半句仍然在逐字增长。"
        });
        let best_before = state.best_transcript_text.clone();
        let endpoint_before = state.local_target_speech_end_ms;

        let visual = display_only_provisional_preview_candidate(&mut state, &provider, true)
            .expect("verified owner uncertainty should permit visual-only cadence");
        assert_eq!(visual, provider["text"]);
        assert_eq!(state.best_transcript_text, best_before);
        assert_eq!(state.local_target_speech_end_ms, endpoint_before);
        assert!(state.optimistic_preview_text.is_empty());

        state.local_speaker_classification = Some(
            crate::speaker_verification::SessionSpeakerClassification::NonTarget { score: 0.1 },
        );
        let longer_foreign = json!({
            "text": "开始录音。这是已确认的主讲人前半句，后半句仍然在逐字增长。旁人说话不得继续显示。"
        });
        assert!(
            display_only_provisional_preview_candidate(&mut state, &longer_foreign, true).is_none(),
            "explicit NonTarget evidence must close the visual-only gate"
        );
    }

    #[test]
    fn installed_session_1868_keeps_owner_preview_monotonic_until_final_or_speaker_switch() {
        let mut state = SyncState {
            local_speaker_tracking_enabled: true,
            local_speaker_stable_target: true,
            local_target_confirmed: true,
            last_emitted_preview_text:
                "开始录音哦所以说你现在已经修好了对吧没有发现新的功能回退了吧因为这个东西准备要卖了"
                    .into(),
            ..SyncState::default()
        };
        let incomplete_two_pass = "开始录音哦所以说你现在已经修好了对吧没有发现新的功能回退了吧";

        assert!(should_preserve_longer_owner_preview(
            &state,
            incomplete_two_pass,
            false,
        ));
        // Protocol final is allowed to make a real correction.
        assert!(!should_preserve_longer_owner_preview(
            &state,
            incomplete_two_pass,
            true,
        ));

        // One transient NonTarget sample does not create UI flicker while the
        // debounced identity remains the owner.
        state.local_speaker_classification = Some(
            crate::speaker_verification::SessionSpeakerClassification::NonTarget { score: 0.32 },
        );
        assert!(should_preserve_longer_owner_preview(
            &state,
            incomplete_two_pass,
            false,
        ));

        // Two confirmed NonTarget samples freeze the owner ledger; the shorter
        // filtered preview must then be allowed to retract foreign speech.
        state.local_speaker_stable_target = false;
        state.owner_isolation_frozen = true;
        assert!(!should_preserve_longer_owner_preview(
            &state,
            incomplete_two_pass,
            false,
        ));
    }

    #[test]
    fn isolation_freeze_blocks_polluted_final_over_owner_ceiling() {
        // After NonTarget restabilizes away from owner, a longer mixed final must
        // not expand the ledger past the freeze ceiling (1.0.4 multi-speaker goal).
        let mut state = SyncState::default();
        state.local_speaker_tracking_enabled = true;
        state.local_speaker_stable_target = false;
        state.local_target_confirmed = true;
        state.local_non_target_speech_end_ms = Some(9_000);
        state.best_transcript_text = "本人说完了".to_string();
        state.owner_isolation_frozen = true;
        state.owner_isolation_ceiling_text = "本人说完了".to_string();
        state.optimistic_preview_text = "本人说完了旁人还在讲很久".to_string();
        let (clamped, _) = clamp_to_owner_isolation_ceiling(
            &state,
            "本人说完了旁人还在讲很久".to_string(),
            Vec::new(),
        );
        assert_eq!(clamped, "本人说完了");
        let fallback = confirmed_owner_final_preview_fallback(&state, "本人说完了");
        // Isolation path must not pick the longer optimistic room text.
        assert!(
            fallback.is_none()
                || spoken_content_len(&fallback.as_ref().unwrap().0)
                    <= spoken_content_len("本人说完了"),
            "final fallback must stay within owner ceiling, got {:?}",
            fallback.as_ref().map(|(t, _)| t.clone())
        );
    }

    #[test]
    fn local_evidence_strict_majority_rejects_non_target_window() {
        let utterance = json!({
            "additions": { "speaker": "0" },
            "definite": true,
            "start_time": 2000,
            "end_time": 5000,
            "text": "旁人插话"
        });
        let evidence = vec![
            LocalSpeakerEvidence {
                audio_end_ms: 2_600,
                classification: crate::speaker_verification::SessionSpeakerClassification::Target {
                    score: 0.5,
                },
                stable_target: true,
            },
            LocalSpeakerEvidence {
                audio_end_ms: 3_400,
                classification:
                    crate::speaker_verification::SessionSpeakerClassification::NonTarget {
                        score: 0.2,
                    },
                stable_target: true, // debounce still holding
            },
            LocalSpeakerEvidence {
                audio_end_ms: 4_200,
                classification:
                    crate::speaker_verification::SessionSpeakerClassification::NonTarget {
                        score: 0.15,
                    },
                stable_target: false,
            },
        ];
        assert!(
            !local_evidence_allows_utterance(&utterance, &evidence, Some("开始录音")),
            "NonTarget-majority window must reject room utterance"
        );
    }

    #[test]
    fn cloud_target_survives_uncertain_and_isolated_non_target_windows() {
        // Installed session 81: the provider attributed the complete utterance
        // to speaker 0, while the local verifier stayed on the owner through
        // mostly Uncertain samples and two isolated noisy NonTarget samples.
        let utterance = json!({
            "additions": { "speaker_id": "0", "source": "two_pass" },
            "definite": true,
            "start_time": 102,
            "end_time": 5762,
            "text": "开始录音。供应商中包修复，必须保留完整正文。"
        });
        let mut evidence = vec![
            LocalSpeakerEvidence {
                audio_end_ms: 2_400,
                classification:
                    crate::speaker_verification::SessionSpeakerClassification::Uncertain {
                        score: 0.424,
                    },
                stable_target: true,
            },
            LocalSpeakerEvidence {
                audio_end_ms: 4_000,
                classification:
                    crate::speaker_verification::SessionSpeakerClassification::Uncertain {
                        score: 0.5404,
                    },
                stable_target: true,
            },
            LocalSpeakerEvidence {
                audio_end_ms: 5_600,
                classification:
                    crate::speaker_verification::SessionSpeakerClassification::NonTarget {
                        score: 0.2116,
                    },
                stable_target: true,
            },
        ];

        assert!(utterance_belongs_to_target(
            &utterance,
            "0",
            true,
            &evidence,
            Some("开始录音"),
            Some(1_000),
            false,
        ));

        evidence.push(LocalSpeakerEvidence {
            audio_end_ms: 5_900,
            classification: crate::speaker_verification::SessionSpeakerClassification::NonTarget {
                score: 0.18,
            },
            stable_target: false,
        });
        assert!(
            !utterance_belongs_to_target(
                &utterance,
                "0",
                true,
                &evidence,
                Some("开始录音"),
                Some(1_000),
                false,
            ),
            "a debounced identity switch must still reject the cloud-owner utterance"
        );
    }

    #[test]
    fn room_speech_raw_stream_does_not_inflate_optimistic_while_local_non_target() {
        // Installed multi-speaker failure (baeff75a-class): cloud keeps one raw
        // stream / speaker "0" growing past the Target-filtered body while local
        // embedding already reports NonTarget. Optimistic must freeze at Target
        // text so the session ledger cannot paste the whole room.
        let result = json!({
            "text": "开始录音本人说完了旁人还在继续讲很久",
            "utterances": [
                {
                    "additions": { "speaker": "0", "source": "two_pass" },
                    "definite": true,
                    "start_time": 100,
                    "end_time": 2600,
                    "text": "开始录音本人说完了"
                },
                {
                    "additions": { "speaker": "0", "source": "stream" },
                    "definite": false,
                    "start_time": 2700,
                    "end_time": 9000,
                    "text": "旁人还在继续讲很久"
                }
            ]
        });
        let evidence = vec![
            LocalSpeakerEvidence {
                audio_end_ms: 2_200,
                classification: crate::speaker_verification::SessionSpeakerClassification::Target {
                    score: 0.55,
                },
                stable_target: true,
            },
            LocalSpeakerEvidence {
                audio_end_ms: 8_500,
                classification:
                    crate::speaker_verification::SessionSpeakerClassification::NonTarget {
                        score: 0.18,
                    },
                // Debounce may still hold owner identity for one window.
                stable_target: true,
            },
        ];
        let mut target = Some("0".to_string());
        let filtered = filter_result_to_target_speaker_with_local_evidence(
            &result,
            &mut target,
            true,
            &evidence,
            Some("开始录音"),
        );
        assert_eq!(filtered.result["text"], "开始录音本人说完了");
        assert_eq!(
            filtered.optimistic_result["text"], "开始录音本人说完了",
            "NonTarget latest sample must freeze optimistic at Target-filtered text"
        );
        assert!(
            spoken_content_len(
                filtered.optimistic_result["text"]
                    .as_str()
                    .unwrap_or_default()
            ) < spoken_content_len("开始录音本人说完了旁人还在继续讲很久"),
            "room tail must not enter optimistic ledger"
        );
    }

    #[test]
    fn local_speaker_negative_evidence_is_endpoint_only_and_never_deletes_text() {
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        asr.note_local_speaker_tracking_started("开始录音");

        asr.note_local_speaker_classification(
            1_000,
            crate::speaker_verification::SessionSpeakerClassification::NonTarget { score: 0.2 },
        );
        {
            let state = asr.state.lock();
            assert!(state.local_speaker_stable_target);
            assert_eq!(state.local_non_target_speech_end_ms, None);
            assert_eq!(state.local_target_speech_end_ms, None);
        }
        asr.note_local_speaker_classification(
            1_400,
            crate::speaker_verification::SessionSpeakerClassification::Uncertain { score: 0.38 },
        );
        asr.note_local_speaker_classification(
            1_800,
            crate::speaker_verification::SessionSpeakerClassification::NonTarget { score: 0.2 },
        );
        assert!(asr.state.lock().local_speaker_stable_target);
        assert_eq!(asr.state.lock().local_target_speech_end_ms, None);
        asr.note_local_speaker_classification(
            2_200,
            crate::speaker_verification::SessionSpeakerClassification::NonTarget { score: 0.2 },
        );
        assert!(asr.state.lock().local_speaker_stable_target);
        assert_eq!(asr.state.lock().local_target_speech_end_ms, None);
        assert_eq!(asr.state.lock().local_non_target_speech_end_ms, Some(2_200));
        assert!(!asr.state.lock().owner_isolation_frozen);
        assert!(matches!(
            asr.state.lock().local_speaker_classification,
            Some(crate::speaker_verification::SessionSpeakerClassification::NonTarget { .. })
        ));

        asr.note_local_speaker_classification(
            2_600,
            crate::speaker_verification::SessionSpeakerClassification::Target { score: 0.55 },
        );
        assert!(asr.state.lock().local_speaker_stable_target);
        assert_eq!(asr.state.lock().local_non_target_speech_end_ms, Some(2_200));
        asr.note_local_speaker_classification(
            3_000,
            crate::speaker_verification::SessionSpeakerClassification::Target { score: 0.55 },
        );
        assert!(asr.state.lock().local_speaker_stable_target);
        assert!(asr.state.lock().local_target_confirmed);
        assert_eq!(asr.state.lock().local_target_speech_end_ms, Some(3_000));
    }

    #[test]
    fn installed_session_2024_keeps_provider_text_when_enrolled_body_scores_are_low() {
        // Production session 2024: Volcengine returned 41 chars twice (live and
        // retained-audio replay) with no utterance rows, while the enrolled
        // session bank scored the same owner at 0.174/0.172. Local identity may
        // guide endpointing, but it cannot turn recognized text into an empty
        // final when the provider has no evidence of a second speaker.
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        asr.note_local_speaker_tracking_started("开始录音");
        asr.state.lock().target_speaker_id = Some("0".into());
        asr.note_local_speaker_classification(
            7_100,
            crate::speaker_verification::SessionSpeakerClassification::NonTarget {
                score: 0.174_005_75,
            },
        );
        asr.note_local_speaker_classification(
            7_500,
            crate::speaker_verification::SessionSpeakerClassification::NonTarget {
                score: 0.172_248_9,
            },
        );
        {
            let state = asr.state.lock();
            assert!(state.local_speaker_stable_target);
            assert!(!state.owner_isolation_frozen);
            assert_eq!(state.local_non_target_speech_end_ms, Some(7_500));
        }

        let provider_text = "开始录音。现在这是一整段能够被服务端正确识别并且必须完整保留的正文。";
        let (tx, mut rx) = oneshot::channel();
        asr.state.lock().final_tx = Some(tx);
        let final_payload = serde_json::to_vec(&json!({
            "audio_info": { "duration": 9_363 },
            "result": {
                "text": provider_text,
                "utterances": []
            }
        }))
        .expect("session 2024 final serializes");
        let final_frame = frame::build(
            MessageType::FullServerResponse,
            Flags::LastPacket,
            Serialization::Json,
            &final_payload,
            None,
        );

        assert!(!asr.handle_frame(&final_frame));
        let transcript = rx
            .try_recv()
            .expect("session 2024 final should resolve")
            .expect("provider-recognized owner text must not become empty");
        assert_eq!(transcript.text, provider_text);
    }

    #[test]
    fn same_cloud_speaker_id_excludes_later_utterance_with_sustained_owner_absence() {
        // Installed session d96a8653: Volcengine emitted two stable utterances
        // but reused speaker 0 for both people. The second utterance had no
        // local Target window and a sustained 0.17..0.30 owner-absence run.
        let result = json!({
            "text": "开始录音。主人正文。旁人的话不能放进去。",
            "utterances": [
                {
                    "additions": { "speaker_id": "0", "source": "two_pass" },
                    "definite": true,
                    "start_time": 0,
                    "end_time": 5_112,
                    "text": "开始录音。主人正文。"
                },
                {
                    "additions": { "speaker_id": "0", "source": "two_pass" },
                    "definite": true,
                    "start_time": 5_700,
                    "end_time": 9_632,
                    "text": "旁人的话不能放进去。"
                }
            ]
        });
        let evidence = vec![
            LocalSpeakerEvidence {
                audio_end_ms: 2_200,
                classification: crate::speaker_verification::SessionSpeakerClassification::Target {
                    score: 0.65,
                },
                stable_target: true,
            },
            LocalSpeakerEvidence {
                audio_end_ms: 6_800,
                classification:
                    crate::speaker_verification::SessionSpeakerClassification::Uncertain {
                        score: 0.29,
                    },
                stable_target: true,
            },
            LocalSpeakerEvidence {
                audio_end_ms: 7_200,
                classification:
                    crate::speaker_verification::SessionSpeakerClassification::Uncertain {
                        score: 0.25,
                    },
                stable_target: true,
            },
            LocalSpeakerEvidence {
                audio_end_ms: 7_600,
                classification:
                    crate::speaker_verification::SessionSpeakerClassification::Uncertain {
                        score: 0.18,
                    },
                stable_target: true,
            },
        ];
        let mut target = None;
        let filtered = filter_result_to_target_speaker_with_local_evidence(
            &result,
            &mut target,
            true,
            &evidence,
            Some("开始录音"),
        );
        assert_eq!(target.as_deref(), Some("0"));
        assert_eq!(filtered.result["text"], "开始录音。主人正文。");
        assert!(filtered.stable_non_target_utterance_present);
    }

    #[test]
    fn verified_wake_excludes_same_cloud_other_without_lucky_body_target() {
        // Field regression: the persisted owner passed the wake gate, but the
        // first body windows never reached Target. A later real person reused
        // cloud speaker 0 and produced a sustained very-low enrolled score.
        // The accepted wake must provide identity authority for this session;
        // requiring a separate lucky body Target lets the other person leak.
        let result = json!({
            "text": "开始录音。主人第一句。旁人的话不能放进去。",
            "utterances": [
                {
                    "additions": { "speaker_id": "0", "source": "two_pass" },
                    "definite": true,
                    "start_time": 0,
                    "end_time": 5_100,
                    "text": "开始录音。主人第一句。"
                },
                {
                    "additions": { "speaker_id": "0", "source": "two_pass" },
                    "definite": true,
                    "start_time": 5_700,
                    "end_time": 9_600,
                    "text": "旁人的话不能放进去。"
                }
            ]
        });
        let evidence = vec![
            LocalSpeakerEvidence {
                audio_end_ms: 6_800,
                classification:
                    crate::speaker_verification::SessionSpeakerClassification::Uncertain {
                        score: 0.19,
                    },
                stable_target: true,
            },
            LocalSpeakerEvidence {
                audio_end_ms: 7_200,
                classification:
                    crate::speaker_verification::SessionSpeakerClassification::Uncertain {
                        score: 0.16,
                    },
                stable_target: true,
            },
            LocalSpeakerEvidence {
                audio_end_ms: 7_600,
                classification:
                    crate::speaker_verification::SessionSpeakerClassification::Uncertain {
                        score: 0.12,
                    },
                stable_target: true,
            },
        ];
        let mut target = None;
        let filtered = filter_result_to_target_speaker_with_local_evidence_and_anchor(
            &result,
            &mut target,
            true,
            &evidence,
            Some("开始录音"),
            None,
            true,
            false,
            Some(7_600),
        );
        assert_eq!(target.as_deref(), Some("0"));
        assert_eq!(filtered.result["text"], "开始录音。主人第一句。");
        assert!(filtered.stable_non_target_utterance_present);

        let mut adaptive_target = None;
        let adaptive = filter_result_to_target_speaker_with_local_evidence_and_anchor(
            &result,
            &mut adaptive_target,
            true,
            &evidence,
            Some("开始录音"),
            None,
            true,
            true,
            Some(7_600),
        );
        assert_eq!(
            adaptive.result["text"], result["text"],
            "a wake-derived adaptive profile must remain non-destructive"
        );
        assert!(!adaptive.stable_non_target_utterance_present);
    }

    #[test]
    fn brief_low_score_dip_does_not_exclude_same_speaker_continuation() {
        let result = json!({
            "text": "开始录音。第一句。停顿以后还是主人第二句。",
            "utterances": [
                {
                    "additions": { "speaker_id": "0", "source": "two_pass" },
                    "definite": true,
                    "start_time": 0,
                    "end_time": 3_200,
                    "text": "开始录音。第一句。"
                },
                {
                    "additions": { "speaker_id": "0", "source": "two_pass" },
                    "definite": true,
                    "start_time": 4_000,
                    "end_time": 6_800,
                    "text": "停顿以后还是主人第二句。"
                }
            ]
        });
        let evidence = vec![
            LocalSpeakerEvidence {
                audio_end_ms: 2_000,
                classification: crate::speaker_verification::SessionSpeakerClassification::Target {
                    score: 0.63,
                },
                stable_target: true,
            },
            LocalSpeakerEvidence {
                audio_end_ms: 5_000,
                classification:
                    crate::speaker_verification::SessionSpeakerClassification::Uncertain {
                        score: 0.24,
                    },
                stable_target: true,
            },
            LocalSpeakerEvidence {
                audio_end_ms: 5_400,
                classification:
                    crate::speaker_verification::SessionSpeakerClassification::Uncertain {
                        score: 0.27,
                    },
                stable_target: true,
            },
        ];
        let mut target = None;
        let filtered = filter_result_to_target_speaker_with_local_evidence(
            &result,
            &mut target,
            true,
            &evidence,
            Some("开始录音"),
        );
        assert_eq!(filtered.result["text"], result["text"]);
        assert!(!filtered.stable_non_target_utterance_present);
    }

    #[test]
    fn installed_session_27_keeps_body_when_tracker_never_calibrates_target() {
        // Installed session 27 delivered every packet and the provider retained
        // the complete 31-character result. The wake gate verified the owner,
        // but every post-wake body window stayed debounced on that identity as
        // Uncertain; none reached Target. A run that never positively calibrated
        // must not reinterpret low Uncertain scores as a destructive speaker
        // switch and leave only the short final tail.
        let result = json!({
            "text": "开始录音。前半段必须保留，停顿以后没什么问题啊。",
            "utterances": [
                {
                    "additions": { "speaker_id": "0", "source": "two_pass" },
                    "definite": true,
                    "start_time": 0,
                    "end_time": 1_402,
                    "text": "开始录音。"
                },
                {
                    "additions": { "speaker_id": "0", "source": "two_pass" },
                    "definite": true,
                    "start_time": 1_500,
                    "end_time": 7_692,
                    "text": "前半段必须保留，停顿以后没什么问题啊。"
                }
            ]
        });
        let evidence = [
            (1_800, 0.344_265),
            (2_200, 0.172_276_29),
            (3_000, 0.432_747_87),
            (4_200, 0.268_078),
            (5_000, 0.308_645_78),
            (6_400, 0.236_441_76),
            (6_800, 0.235_123_95),
        ]
        .into_iter()
        .map(|(audio_end_ms, score)| LocalSpeakerEvidence {
            audio_end_ms,
            classification: crate::speaker_verification::SessionSpeakerClassification::Uncertain {
                score,
            },
            stable_target: true,
        })
        .collect::<Vec<_>>();
        let mut target = None;

        let filtered = filter_result_to_target_speaker_with_local_evidence(
            &result,
            &mut target,
            true,
            &evidence,
            Some("开始录音"),
        );

        assert_eq!(target.as_deref(), Some("0"));
        assert_eq!(filtered.result["text"], result["text"]);
        assert!(!filtered.stable_non_target_utterance_present);
    }

    #[test]
    fn installed_session_52_keeps_post_pause_tail_inside_one_wake_anchored_utterance() {
        // Installed acceptance session 52 uploaded 14.64 s with zero packet
        // loss. Volcengine returned the complete prompt as one stable speaker-0
        // utterance, but the local verifier stayed debounced on the owner while
        // producing low-score Uncertain windows. Those windows must not erase
        // the entire continuation merely because the provider did not split it
        // into a second utterance at the pause.
        let owner_text = "开始录音。请把蓝牙配对和自动结束这两个问题一起检查，然后告诉我检查结果。我现在停顿一下，继续说最后一句：这次内容只能出现一遍，不能凭空重复前面的句子。";
        let result = json!({
            "text": owner_text,
            "utterances": [{
                "additions": { "speaker_id": "0", "source": "two_pass" },
                "definite": true,
                "start_time": 0,
                "end_time": 12_832,
                "text": owner_text
            }]
        });
        let evidence = [
            (1_000, 0.513_517_26),
            (6_500, 0.244_018_81),
            (8_600, 0.220_224_95),
            (9_000, 0.229_116_51),
            (10_300, 0.235_262_33),
            (10_800, 0.208_365_05),
            (11_200, 0.217_355_97),
            (12_000, 0.308_844_1),
        ]
        .into_iter()
        .map(|(audio_end_ms, score)| LocalSpeakerEvidence {
            audio_end_ms,
            classification: crate::speaker_verification::SessionSpeakerClassification::Uncertain {
                score,
            },
            stable_target: true,
        })
        .collect::<Vec<_>>();
        let mut target = None;

        let filtered = filter_result_to_target_speaker_with_local_evidence(
            &result,
            &mut target,
            true,
            &evidence,
            Some("开始录音"),
        );

        assert_eq!(target.as_deref(), Some("0"));
        assert_eq!(filtered.result["text"], owner_text);
        assert_eq!(filtered.target_speech_end_ms, Some(12_832));
        assert!(!filtered.stable_non_target_utterance_present);

        // The exemption is only for a debounced owner. A real identity switch
        // inside the same provider utterance remains hard exclusion evidence.
        let mut switched_evidence = evidence;
        switched_evidence.push(LocalSpeakerEvidence {
            audio_end_ms: 12_400,
            classification: crate::speaker_verification::SessionSpeakerClassification::NonTarget {
                score: 0.18,
            },
            stable_target: false,
        });
        let mut switched_target = Some("0".to_string());
        let switched = filter_result_to_target_speaker_with_local_evidence(
            &result,
            &mut switched_target,
            true,
            &switched_evidence,
            Some("开始录音"),
        );
        assert_eq!(switched.result["text"], "");
        assert!(switched.stable_non_target_utterance_present);
    }

    #[test]
    fn protocol_final_cannot_restore_same_cluster_other_speaker_tail() {
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        asr.note_local_speaker_tracking_started("开始录音");
        asr.note_local_speaker_profile_adaptive(true);
        asr.note_local_speaker_classification(
            2_200,
            crate::speaker_verification::SessionSpeakerClassification::Target { score: 0.65 },
        );
        for (audio_end_ms, score) in [(6_800, 0.29), (7_200, 0.25), (7_600, 0.18)] {
            asr.note_local_speaker_classification(
                audio_end_ms,
                crate::speaker_verification::SessionSpeakerClassification::Uncertain { score },
            );
        }
        {
            let mut state = asr.state.lock();
            state.best_transcript_text = "开始录音。主人正文。旁人的话不能放进去。".into();
            state.last_partial_text = state.best_transcript_text.clone();
            state.optimistic_preview_text = state.best_transcript_text.clone();
        }
        let provider_result = json!({
            "text": "开始录音。主人正文。旁人的话不能放进去。",
            "utterances": [
                {
                    "additions": { "speaker_id": "0", "source": "two_pass" },
                    "definite": true,
                    "start_time": 0,
                    "end_time": 5_112,
                    "text": "开始录音。主人正文。"
                },
                {
                    "additions": { "speaker_id": "0", "source": "two_pass" },
                    "definite": true,
                    "start_time": 5_700,
                    "end_time": 9_632,
                    "text": "旁人的话不能放进去。"
                }
            ]
        });
        let (tx, mut rx) = oneshot::channel();
        asr.state.lock().final_tx = Some(tx);
        let payload = serde_json::to_vec(&json!({
            "audio_info": { "duration": 10_100 },
            "result": provider_result,
        }))
        .expect("same-cluster final serializes");
        let frame = frame::build(
            MessageType::FullServerResponse,
            Flags::LastPacket,
            Serialization::Json,
            &payload,
            None,
        );
        assert!(!asr.handle_frame(&frame));
        let transcript = rx
            .try_recv()
            .expect("same-cluster final should resolve")
            .expect("owner text should remain non-empty");
        assert_eq!(transcript.text, "开始录音。主人正文。");
        assert!(asr.state.lock().owner_isolation_frozen);
    }

    #[test]
    fn owner_can_resume_after_provider_local_isolation_freeze() {
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        asr.note_local_speaker_tracking_started("开始录音");
        asr.state.lock().owner_isolation_frozen = true;
        asr.note_local_speaker_classification(
            8_000,
            crate::speaker_verification::SessionSpeakerClassification::Target { score: 0.66 },
        );
        assert!(asr.state.lock().owner_isolation_frozen);
        asr.note_local_speaker_classification(
            8_400,
            crate::speaker_verification::SessionSpeakerClassification::Target { score: 0.68 },
        );
        assert!(!asr.state.lock().owner_isolation_frozen);
    }

    #[test]
    fn ephemeral_wake_profile_cannot_freeze_or_delete_same_session_body() {
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        asr.note_local_speaker_tracking_started("开始录音");
        asr.note_local_speaker_profile_adaptive(true);

        for (audio_end_ms, score) in [(3_700, 0.3103), (4_100, 0.3190), (4_500, 0.2742)] {
            asr.note_local_speaker_classification(
                audio_end_ms,
                crate::speaker_verification::SessionSpeakerClassification::NonTarget { score },
            );
        }

        let state = asr.state.lock();
        assert!(state.local_speaker_stable_target);
        assert!(!state.owner_isolation_frozen);
        assert_eq!(state.local_consecutive_non_target, 0);
        assert_eq!(state.local_non_target_speech_end_ms, None);
        assert!(state.local_speaker_evidence.iter().all(|sample| matches!(
            sample.classification,
            crate::speaker_verification::SessionSpeakerClassification::Uncertain { .. }
        )));
    }

    #[test]
    fn adaptive_profile_marks_repeated_strong_other_speech_for_endpoint_only() {
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        asr.note_local_speaker_tracking_started("开始录音");
        asr.note_local_speaker_profile_adaptive(true);
        asr.note_local_speaker_classification(
            12_600,
            crate::speaker_verification::SessionSpeakerClassification::NonTarget { score: 0.089 },
        );
        assert_eq!(asr.state.lock().local_non_target_speech_end_ms, None);
        asr.note_local_speaker_classification(
            13_000,
            crate::speaker_verification::SessionSpeakerClassification::NonTarget { score: 0.112 },
        );
        {
            let state = asr.state.lock();
            assert_eq!(state.local_non_target_speech_end_ms, Some(13_000));
            assert_eq!(state.local_sustained_non_target_speech_end_ms, None);
            assert_eq!(
                target_speaker_update_from_state(&state, false, false)
                    .local_non_target_speech_end_ms,
                None,
                "two strong windows alone remain advisory for endpointing"
            );
        }
        asr.note_local_speaker_classification(
            13_400,
            crate::speaker_verification::SessionSpeakerClassification::Uncertain { score: 0.24 },
        );

        let state = asr.state.lock();
        assert!(state.local_speaker_stable_target);
        assert!(!state.owner_isolation_frozen);
        assert_eq!(state.local_non_target_speech_end_ms, Some(13_400));
        assert_eq!(state.local_sustained_non_target_speech_end_ms, Some(13_400));
        assert_eq!(
            target_speaker_update_from_state(&state, false, false).local_non_target_speech_end_ms,
            Some(13_400)
        );
        assert!(matches!(
            state.local_speaker_classification,
            Some(crate::speaker_verification::SessionSpeakerClassification::Uncertain { .. })
        ));
    }

    #[test]
    fn startup_false_negatives_do_not_create_target_endpoint_authority() {
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        asr.note_local_speaker_tracking_started("开始录音");

        for (audio_end_ms, classification) in [
            (
                1_900,
                crate::speaker_verification::SessionSpeakerClassification::NonTarget {
                    score: 0.192,
                },
            ),
            (
                2_300,
                crate::speaker_verification::SessionSpeakerClassification::NonTarget {
                    score: 0.188,
                },
            ),
            (
                2_700,
                crate::speaker_verification::SessionSpeakerClassification::Uncertain {
                    score: 0.371,
                },
            ),
            (
                3_100,
                crate::speaker_verification::SessionSpeakerClassification::Uncertain {
                    score: 0.389,
                },
            ),
        ] {
            asr.note_local_speaker_classification(audio_end_ms, classification);
        }
        {
            let state = asr.state.lock();
            assert!(state.local_speaker_stable_target);
            assert!(!state.local_target_confirmed);
            assert_eq!(state.local_target_speech_end_ms, None);
            assert_eq!(state.local_non_target_speech_end_ms, Some(2_300));
            assert_eq!(state.local_sustained_non_target_speech_end_ms, None);
            assert!(!state.owner_isolation_frozen);
        }

        asr.note_local_speaker_classification(
            3_500,
            crate::speaker_verification::SessionSpeakerClassification::Target { score: 0.50 },
        );
        let state = asr.state.lock();
        assert!(state.local_speaker_stable_target);
        assert!(state.local_target_confirmed);
        assert_eq!(state.local_target_speech_end_ms, Some(3_500));
    }

    #[test]
    fn transient_non_target_does_not_extend_owner_endpoint_clock() {
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        asr.note_local_speaker_tracking_started("开始录音");

        for (audio_end_ms, score) in [(2_500, 0.495), (2_900, 0.499), (3_600, 0.559)] {
            asr.note_local_speaker_classification(
                audio_end_ms,
                crate::speaker_verification::SessionSpeakerClassification::Target { score },
            );
        }
        // First NonTarget keeps debounced identity, but must not pretend the
        // owner is still speaking for auto-end timing.
        asr.note_local_speaker_classification(
            5_200,
            crate::speaker_verification::SessionSpeakerClassification::NonTarget { score: 0.095 },
        );

        let state = asr.state.lock();
        assert!(state.local_speaker_stable_target);
        assert_eq!(state.local_consecutive_non_target, 0);
        assert_eq!(state.local_consecutive_strong_non_target, 1);
        assert_eq!(state.local_target_speech_end_ms, Some(3_600));
        assert_eq!(state.local_non_target_speech_end_ms, None);
    }

    #[test]
    fn uncertain_while_stable_owner_does_not_refresh_endpoint_clock() {
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        asr.note_local_speaker_tracking_started("开始录音");
        asr.note_local_speaker_classification(
            1_800,
            crate::speaker_verification::SessionSpeakerClassification::Target { score: 0.80 },
        );
        asr.note_local_speaker_classification(
            2_200,
            crate::speaker_verification::SessionSpeakerClassification::Uncertain { score: 0.38 },
        );
        let state = asr.state.lock();
        assert!(state.local_speaker_stable_target);
        // LST-REC-031: Uncertain is not owner evidence and cannot refresh the
        // endpoint clock. Target-gated preview growth covers a continuing owner.
        assert_eq!(state.local_target_speech_end_ms, Some(1_800));
    }

    #[test]
    fn owner_preview_growth_refreshes_local_target_while_stable() {
        let mut state = SyncState {
            local_speaker_tracking_enabled: true,
            local_speaker_stable_target: true,
            local_target_confirmed: true,
            // `local_speaker_allows_optimistic_preview` gates refresh on the
            // latest classification being Target (matches the only call site).
            local_speaker_classification: Some(
                crate::speaker_verification::SessionSpeakerClassification::Target { score: 0.6 },
            ),
            local_target_speech_end_ms: Some(1_800),
            local_audio_duration_ms: Some(2_800),
            last_server_audio_duration_ms: Some(2_600),
            ..Default::default()
        };
        assert!(refresh_local_target_from_owner_preview_activity(&mut state));
        assert_eq!(state.local_target_speech_end_ms, Some(2_800));
        // Once flipped away from the owner, preview growth must not lengthen.
        state.local_speaker_stable_target = false;
        state.local_audio_duration_ms = Some(4_000);
        assert!(!refresh_local_target_from_owner_preview_activity(
            &mut state
        ));
        assert_eq!(state.local_target_speech_end_ms, Some(2_800));
    }

    #[test]
    fn uncertain_sequential_provider_split_cannot_refresh_owner_endpoint() {
        // Installed session 7942d10e exposed a circular hold: cloud speaker
        // A/B text was admitted using debounced Uncertain windows, then that
        // preview refreshed the owner clock as if identity were confirmed.
        // Text recovery may remain available, but only a current Target sample
        // can extend the owner endpoint.
        let mut state = SyncState {
            local_speaker_tracking_enabled: true,
            local_speaker_stable_target: true,
            local_target_confirmed: true,
            local_speaker_classification: Some(
                crate::speaker_verification::SessionSpeakerClassification::Uncertain {
                    score: 0.37,
                },
            ),
            local_target_speech_end_ms: Some(4_922),
            local_audio_duration_ms: Some(8_940),
            last_server_audio_duration_ms: Some(8_552),
            ..Default::default()
        };

        assert!(!refresh_local_target_from_owner_safe_provider_split(
            &mut state
        ));
        assert_eq!(state.local_target_speech_end_ms, Some(4_922));

        state.local_speaker_classification =
            Some(crate::speaker_verification::SessionSpeakerClassification::Target { score: 0.62 });
        assert!(refresh_local_target_from_owner_safe_provider_split(
            &mut state
        ));
        assert_eq!(state.local_target_speech_end_ms, Some(8_940));
    }

    #[test]
    fn other_person_speech_does_not_lengthen_owner_auto_end() {
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        asr.note_local_speaker_tracking_started("开始录音");
        for audio_end_ms in [2_000, 2_400, 2_800] {
            asr.note_local_speaker_classification(
                audio_end_ms,
                crate::speaker_verification::SessionSpeakerClassification::Target { score: 0.62 },
            );
        }
        // Two consecutive NonTarget windows (other person talking).
        asr.note_local_speaker_classification(
            3_600,
            crate::speaker_verification::SessionSpeakerClassification::NonTarget { score: 0.18 },
        );
        asr.note_local_speaker_classification(
            4_800,
            crate::speaker_verification::SessionSpeakerClassification::NonTarget { score: 0.15 },
        );
        let state = asr.state.lock();
        assert!(state.local_speaker_stable_target);
        assert!(!state.owner_isolation_frozen);
        assert_eq!(state.local_target_speech_end_ms, Some(2_800));
        assert_eq!(state.local_non_target_speech_end_ms, Some(4_800));
    }

    #[test]
    fn local_wake_speaker_evidence_binds_cloud_target_after_other_speaker() {
        let result = json!({
            "text": "旁人先说目标正文",
            "utterances": [
                {
                    "additions": { "speaker_id": "0", "source": "two_pass" },
                    "definite": true,
                    "start_time": 0,
                    "end_time": 1000,
                    "text": "旁人先说"
                },
                {
                    "additions": { "speaker_id": "1", "source": "two_pass" },
                    "definite": true,
                    "start_time": 2200,
                    "end_time": 4000,
                    "text": "目标正文"
                }
            ]
        });
        let evidence = vec![
            LocalSpeakerEvidence {
                audio_end_ms: 800,
                classification:
                    crate::speaker_verification::SessionSpeakerClassification::NonTarget {
                        score: 0.2,
                    },
                stable_target: false,
            },
            LocalSpeakerEvidence {
                audio_end_ms: 2_800,
                classification: crate::speaker_verification::SessionSpeakerClassification::Target {
                    score: 0.6,
                },
                stable_target: true,
            },
            LocalSpeakerEvidence {
                audio_end_ms: 3_400,
                classification: crate::speaker_verification::SessionSpeakerClassification::Target {
                    score: 0.6,
                },
                stable_target: true,
            },
        ];
        let mut target = None;

        let filtered = filter_result_to_target_speaker_with_local_evidence(
            &result,
            &mut target,
            true,
            &evidence,
            None,
        );

        assert_eq!(target.as_deref(), Some("1"));
        assert_eq!(filtered.result["text"], "目标正文");
    }

    #[test]
    fn stable_wake_phrase_speaker_overrides_misleading_local_embedding() {
        let result = json!({
            "text": "旁人正文开始录音目标正文",
            "utterances": [
                {
                    "additions": { "speaker_id": "0", "source": "two_pass" },
                    "definite": true,
                    "start_time": 0,
                    "end_time": 1200,
                    "text": "旁人正文"
                },
                {
                    "additions": { "speaker_id": "1", "source": "two_pass" },
                    "definite": true,
                    "start_time": 200,
                    "end_time": 2600,
                    "text": "开始录音目标正文"
                }
            ]
        });
        let misleading_evidence = vec![LocalSpeakerEvidence {
            audio_end_ms: 1_000,
            classification: crate::speaker_verification::SessionSpeakerClassification::Target {
                score: 0.7,
            },
            stable_target: true,
        }];
        let mut target = None;

        let filtered = filter_result_to_target_speaker_with_local_evidence(
            &result,
            &mut target,
            true,
            &misleading_evidence,
            Some("开始录音"),
        );

        assert_eq!(target.as_deref(), Some("1"));
        assert_eq!(filtered.result["text"], "开始录音目标正文");
    }

    #[test]
    fn local_timeline_recovers_body_when_cloud_misrecognizes_wake_and_splits_speaker() {
        // 2026-08-10 installed first-try failure: the local gate confirmed
        // "开始录音", but cloud rendered the wake as "此录音" and assigned
        // the owner's following body to a second diarization cluster. The
        // cloud response had valid text while the target filter returned empty.
        let result = json!({
            "text": "此录音。首次命中复验只说这一次。",
            "utterances": [
                {
                    "additions": { "speaker_id": "0", "source": "two_pass" },
                    "definite": true,
                    "start_time": 40,
                    "end_time": 982,
                    "text": "此录音。"
                },
                {
                    "additions": { "speaker_id": "1", "source": "two_pass" },
                    "definite": true,
                    "start_time": 1100,
                    "end_time": 3962,
                    "text": "首次命中复验只说这一次。"
                }
            ]
        });
        let evidence = vec![
            LocalSpeakerEvidence {
                audio_end_ms: 1_900,
                classification: crate::speaker_verification::SessionSpeakerClassification::Target {
                    score: 0.56,
                },
                stable_target: true,
            },
            LocalSpeakerEvidence {
                audio_end_ms: 5_800,
                classification:
                    crate::speaker_verification::SessionSpeakerClassification::NonTarget {
                        score: 0.19,
                    },
                stable_target: true,
            },
        ];
        let mut target = None;

        let filtered = filter_result_to_target_speaker_with_local_evidence(
            &result,
            &mut target,
            true,
            &evidence,
            Some("开始录音"),
        );

        assert_eq!(target.as_deref(), Some("0"));
        assert_eq!(filtered.result["text"], "首次命中复验只说这一次。");
        assert_eq!(filtered.target_speech_end_ms, Some(3962));
    }

    #[test]
    fn installed_b6c7bd31_keeps_immediate_body_cluster_across_rolling_responses() {
        // Production rolling responses first sealed the verified wake as
        // speaker 0, then exposed only one growing speaker-1 body utterance in
        // every later packet. Cross-phrase voiceprint scores stayed Uncertain,
        // so requiring a fresh positive Target vote froze preview at 17 chars
        // even though the body began 68 ms after the wake row.
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        asr.note_local_speaker_tracking_started("开始录音");

        let wake_payload = serde_json::to_vec(&json!({
            "audio_info": { "duration": 1_800 },
            "result": {
                "text": "开始录音。",
                "utterances": [{
                    "additions": { "speaker_id": "0", "source": "two_pass" },
                    "definite": true,
                    "start_time": 0,
                    "end_time": 1_032,
                    "text": "开始录音。"
                }]
            }
        }))
        .expect("wake response serializes");
        let wake_frame = frame::build(
            MessageType::FullServerResponse,
            Flags::None,
            Serialization::Json,
            &wake_payload,
            None,
        );
        assert!(asr.handle_frame(&wake_frame));
        assert_eq!(asr.state.lock().wake_target_speech_end_ms, Some(1_032));

        for (audio_end_ms, score) in [
            (2_500, 0.330_394_54),
            (3_300, 0.358_282_4),
            (5_700, 0.281_194_36),
            (9_400, 0.315_429_87),
        ] {
            asr.note_local_speaker_classification(
                audio_end_ms,
                crate::speaker_verification::SessionSpeakerClassification::Uncertain { score },
            );
        }

        let full_text =
            "开始录音。帮我看一下这个东西，胶囊弹得快不快，然后预览会卡，然后吞下半截字。";
        let body_text = "帮我看一下这个东西，胶囊弹得快不快，然后预览会卡，然后吞下半截字。";
        let body_result = json!({
            "text": full_text,
            "utterances": [{
                "additions": { "speaker_id": "1", "source": "two_pass" },
                "definite": true,
                "start_time": 1_100,
                "end_time": 9_862,
                "text": body_text
            }]
        });
        let mut target = Some("0".to_string());
        let evidence = asr.state.lock().local_speaker_evidence.clone();
        let filtered = filter_result_to_target_speaker_with_local_evidence_and_anchor(
            &body_result,
            &mut target,
            true,
            &evidence,
            Some("开始录音"),
            Some(1_032),
            false,
            false,
            None,
        );
        assert_eq!(filtered.result["text"], body_text);
        assert_eq!(filtered.target_speech_end_ms, Some(9_862));

        let previews = Arc::new(ParkingMutex::new(Vec::new()));
        let callback_previews = Arc::clone(&previews);
        asr.set_partial_transcript_callback(Some(Arc::new(move |text| {
            callback_previews.lock().push(text);
        })));
        let body_payload = serde_json::to_vec(&json!({
            "audio_info": { "duration": 10_200 },
            "result": body_result
        }))
        .expect("body response serializes");
        let body_frame = frame::build(
            MessageType::FullServerResponse,
            Flags::None,
            Serialization::Json,
            &body_payload,
            None,
        );
        assert!(asr.handle_frame(&body_frame));
        assert!(
            previews
                .lock()
                .iter()
                .any(|preview| preview.contains("吞下半截字")),
            "rolling body cluster must keep advancing the capsule preview"
        );

        let late_other = json!({
            "text": "旁人后来插话",
            "utterances": [{
                "additions": { "speaker_id": "2", "source": "two_pass" },
                "definite": true,
                "start_time": 11_000,
                "end_time": 12_500,
                "text": "旁人后来插话"
            }]
        });
        let rejected = filter_result_to_target_speaker_with_local_evidence_and_anchor(
            &late_other,
            &mut target,
            true,
            &evidence,
            Some("开始录音"),
            Some(1_032),
            false,
            false,
            None,
        );
        assert_eq!(rejected.result["text"], "");
    }

    #[test]
    fn final_wake_only_provider_gap_recovers_long_owner_text_after_uncertain_windows() {
        // Installed sessions 27/40: the provider final had one stable bounded
        // wake utterance while result.text carried the entire body. Local
        // samples were mostly Uncertain but never debounced away from the
        // wake speaker, so the final must retain the already-admitted raw tail.
        let result = json!({
            "text": "开始录音。你抓一下日志，跟着那个波全程在动。",
            "utterances": [{
                "additions": { "speaker_id": "0", "source": "two_pass" },
                "definite": true,
                "start_time": 80,
                "end_time": 1492,
                "text": "开始录音。"
            }]
        });
        let evidence = vec![LocalSpeakerEvidence {
            audio_end_ms: 12_200,
            classification: crate::speaker_verification::SessionSpeakerClassification::Uncertain {
                score: 0.42,
            },
            stable_target: true,
        }];
        let mut target = None;
        let filtered = filter_result_to_target_speaker_with_local_evidence(
            &result,
            &mut target,
            true,
            &evidence,
            Some("开始录音"),
        );
        assert_eq!(filtered.result["text"], "开始录音。");
        assert_eq!(
            filtered.optimistic_result["text"],
            "开始录音。你抓一下日志，跟着那个波全程在动。"
        );

        let mut state = SyncState::default();
        state.local_speaker_tracking_enabled = true;
        state.local_speaker_stable_target = true;
        state.local_speaker_classification = Some(
            crate::speaker_verification::SessionSpeakerClassification::Uncertain { score: 0.42 },
        );
        // Session 52 advanced stable attribution to 5872 ms before the final
        // packet regressed its only utterance back to the 1552 ms wake row.
        state.stable_attributed_speech_end_ms = Some(5_872);
        state.wake_speaker_phrase = Some("开始录音".into());
        assert!(final_wake_only_provider_gap_is_owner_safe(
            &state,
            &result,
            filtered.result["text"].as_str().unwrap(),
        ));

        state.local_consecutive_non_target = 1;
        assert!(
            !final_wake_only_provider_gap_is_owner_safe(
                &state,
                &result,
                filtered.result["text"].as_str().unwrap(),
            ),
            "a current identity-switch streak must keep the raw tail fail-closed"
        );

        // Installed session 86: no enrolled voiceprint existed, so the local
        // profile was derived from 700 ms of wake speech. It then mislabeled
        // the same user's body as NonTarget while the provider retained the
        // complete body in result.text. An ephemeral profile is advisory in
        // this exact wake-only schema gap and must not turn recognized text
        // into an empty result.
        state.local_speaker_profile_adaptive = true;
        state.local_speaker_stable_target = false;
        state.owner_isolation_frozen = true;
        assert!(final_wake_only_provider_gap_is_owner_safe(
            &state,
            &result,
            filtered.result["text"].as_str().unwrap(),
        ));

        state.local_speaker_profile_adaptive = false;
        state.local_speaker_stable_target = true;
        state.owner_isolation_frozen = false;
        state.local_consecutive_non_target = 0;
        state.stable_attributed_speech_end_ms = Some(1_552);
        assert!(
            !final_wake_only_provider_gap_is_owner_safe(
                &state,
                &result,
                filtered.result["text"].as_str().unwrap(),
            ),
            "a wake-only history must not promote an unattributed raw tail"
        );
    }

    #[test]
    fn installed_session_768_recovers_raw_body_without_prior_stable_attribution() {
        // The provider final carried the full sentence in result.text while its
        // only stable utterance remained the wake phrase. All local windows were
        // advisory Uncertain and no repeated strong NonTarget switch existed.
        let result = json!({
            "text": "开始录音。现在更差了，你看一下现在是这样子。",
            "utterances": [{
                "additions": { "speaker_id": "0", "source": "two_pass" },
                "definite": true,
                "start_time": 40,
                "end_time": 1942,
                "text": "开始录音。"
            }]
        });
        let mut state = SyncState::default();
        state.local_speaker_tracking_enabled = true;
        state.local_speaker_profile_adaptive = true;
        state.local_speaker_stable_target = true;
        state.local_speaker_classification = Some(
            crate::speaker_verification::SessionSpeakerClassification::Uncertain {
                score: 0.476_475_06,
            },
        );
        state.target_speaker_id = Some("0".into());
        state.stable_attributed_speech_end_ms = Some(1_942);
        state.wake_speaker_phrase = Some("开始录音".into());
        state.best_transcript_text = "开始录音。".into();
        state.last_partial_text = state.best_transcript_text.clone();

        assert!(final_wake_only_provider_gap_is_owner_safe(
            &state, &result, "",
        ));

        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        {
            let mut runtime = asr.state.lock();
            runtime.local_speaker_tracking_enabled = true;
            runtime.local_speaker_profile_adaptive = true;
            runtime.local_speaker_stable_target = true;
            runtime.local_speaker_classification = state.local_speaker_classification;
            runtime.target_speaker_id = Some("0".into());
            runtime.stable_attributed_speech_end_ms = Some(1_942);
            runtime.wake_speaker_phrase = Some("开始录音".into());
            runtime.best_transcript_text = "开始录音。".into();
            runtime.last_partial_text = runtime.best_transcript_text.clone();
        }
        let (tx, mut rx) = oneshot::channel();
        asr.state.lock().final_tx = Some(tx);
        let final_payload = serde_json::to_vec(&json!({
            "audio_info": { "duration": 7_516 },
            "result": result.clone(),
        }))
        .expect("session 768 final serializes");
        let final_frame = frame::build(
            MessageType::FullServerResponse,
            Flags::LastPacket,
            Serialization::Json,
            &final_payload,
            None,
        );
        assert!(!asr.handle_frame(&final_frame));
        let transcript = rx
            .try_recv()
            .expect("session 768 final should resolve")
            .expect("wake-only provider schema gap should recover body");
        assert_eq!(
            transcript.text,
            "开始录音。现在更差了，你看一下现在是这样子。"
        );

        state.local_non_target_speech_end_ms = Some(4_900);
        assert!(
            !final_wake_only_provider_gap_is_owner_safe(&state, &result, "",),
            "repeated strong local NonTarget evidence must still veto recovery"
        );
    }

    #[test]
    fn sequential_cloud_speaker_split_recovers_for_adaptive_and_enrolled_profiles() {
        // Installed session 27 delivered every audio packet, and the provider
        // returned the complete text, but cloud diarization moved the owner's
        // final sentence from speaker 0 to speaker 1. The adaptive wake profile
        // remained stably on the owner throughout the sequential continuation.
        let result = json!({
            "text": "开始录音。再检查一下整个东西还有没有什么别的问题。比如说录音、绘画日志之类的，然后我再说一下，没什么问题就这样吧，这个产品现在浏览好像卡住了。",
            "utterances": [
                {
                    "additions": { "speaker_id": "0", "source": "two_pass" },
                    "definite": true,
                    "start_time": 40,
                    "end_time": 1172,
                    "text": "开始录音。"
                },
                {
                    "additions": { "speaker_id": "1", "source": "two_pass" },
                    "definite": true,
                    "start_time": 1350,
                    "end_time": 6000,
                    "text": "再检查一下整个东西还有没有什么别的问题。"
                },
                {
                    "additions": { "speaker_id": "0", "source": "two_pass" },
                    "definite": true,
                    "start_time": 6200,
                    "end_time": 8692,
                    "text": "比如说录音、绘画日志之类的，然后我再"
                },
                {
                    "additions": { "speaker_id": "1", "source": "two_pass" },
                    "definite": true,
                    "start_time": 8900,
                    "end_time": 14222,
                    "text": "说一下，没什么问题就这样吧，这个产品现在浏览好像卡住了。"
                }
            ]
        });
        let target_text = "开始录音。比如说录音、绘画日志之类的，然后我再";
        let mut state = SyncState::default();
        state.local_speaker_tracking_enabled = true;
        state.local_speaker_profile_adaptive = true;
        state.local_speaker_stable_target = true;
        state.target_speaker_id = Some("0".into());
        state.wake_speaker_phrase = Some("开始录音".into());
        state.local_speaker_evidence = [
            1_000, 2_000, 4_000, 6_000, 7_000, 8_600, 9_800, 11_200, 13_000, 14_600,
        ]
        .into_iter()
        .map(|audio_end_ms| LocalSpeakerEvidence {
            audio_end_ms,
            classification: crate::speaker_verification::SessionSpeakerClassification::Uncertain {
                score: 0.41,
            },
            stable_target: true,
        })
        .collect();

        assert!(sequential_speaker_split_gap_is_owner_safe(
            &state,
            &result,
            target_text,
        ));

        // Installed session 1195: two sensitive endpoint-only NonTarget votes
        // occurred mid-body, owner-compatible windows recovered afterward, and
        // the raw final classification came from quiet room noise after the
        // provider's last word. Neither may globally erase the sequential body.
        state.local_non_target_speech_end_ms = Some(8_000);
        state.local_speaker_evidence.push(LocalSpeakerEvidence {
            audio_end_ms: 16_000,
            classification: crate::speaker_verification::SessionSpeakerClassification::Uncertain {
                score: 0.10,
            },
            stable_target: true,
        });
        state.local_speaker_classification = Some(
            crate::speaker_verification::SessionSpeakerClassification::NonTarget { score: 0.10 },
        );
        assert!(sequential_speaker_split_gap_is_owner_safe(
            &state,
            &result,
            target_text,
        ));
        state.local_speaker_profile_adaptive = false;
        assert!(sequential_speaker_split_gap_is_owner_safe(
            &state,
            &result,
            target_text,
        ));

        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        *asr.state.lock() = state;
        let (tx, mut rx) = oneshot::channel();
        asr.state.lock().final_tx = Some(tx);
        let final_payload = serde_json::to_vec(&json!({
            "audio_info": { "duration": 15_700 },
            "result": result,
        }))
        .expect("session 27 final serializes");
        let final_frame = frame::build(
            MessageType::FullServerResponse,
            Flags::LastPacket,
            Serialization::Json,
            &final_payload,
            None,
        );

        assert!(!asr.handle_frame(&final_frame));
        let transcript = rx
            .try_recv()
            .expect("session 27 final should resolve")
            .expect("sequential owner continuation should remain successful");
        assert_eq!(
            transcript.text,
            "开始录音。再检查一下整个东西还有没有什么别的问题。比如说录音、绘画日志之类的，然后我再说一下，没什么问题就这样吧，这个产品现在浏览好像卡住了。"
        );
    }

    #[test]
    fn installed_session_621_keeps_one_owner_split_across_three_cloud_ids() {
        // Installed session 621 was one continuous verified-owner utterance,
        // but the provider relabelled it 0 -> 1 -> 2 -> 2. CAM++ recognized
        // only the first windows as Target and left the natural-speech body in
        // the Uncertain band. Cloud cluster identity is not a persistent human
        // identity, so that shape must preserve the complete provider text
        // while every local window still holds the debounced owner identity.
        let full = "开始录音。会有时候会吞字，就很奇怪，是不是这个模型不太行，就是为了避免这个多人识别，然后把自己也变成第二个人了，这个要修一下，体验太差了。";
        let result = json!({
            "text": full,
            "utterances": [
                {
                    "additions": { "speaker_id": "0", "source": "two_pass" },
                    "definite": true,
                    "start_time": 172,
                    "end_time": 1_012,
                    "text": "开始录音。"
                },
                {
                    "additions": { "speaker_id": "1", "source": "two_pass" },
                    "definite": true,
                    "start_time": 1_172,
                    "end_time": 1_782,
                    "text": "会有时候会吞字，"
                },
                {
                    "additions": { "speaker_id": "2", "source": "two_pass" },
                    "definite": true,
                    "start_time": 1_852,
                    "end_time": 4_262,
                    "text": "就很奇怪，是不是这个模型不太行，就是为了避免这个多人识别，"
                },
                {
                    "additions": { "speaker_id": "2", "source": "two_pass" },
                    "definite": true,
                    "start_time": 4_262,
                    "end_time": 8_632,
                    "text": "然后把自己也变成第二个人了，这个要修一下，体验太差了。"
                }
            ]
        });
        let mut state = SyncState {
            local_speaker_tracking_enabled: true,
            local_wake_owner_verified: true,
            local_speaker_profile_adaptive: false,
            local_speaker_stable_target: true,
            local_target_confirmed: true,
            target_speaker_id: Some("0".into()),
            wake_speaker_phrase: Some("开始录音".into()),
            ..SyncState::default()
        };
        state.local_speaker_evidence = [
            (1_200, 0.808),
            (1_600, 0.787),
            (2_200, 0.48),
            (3_000, 0.41),
            (4_200, 0.37),
            (5_400, 0.44),
            (6_600, 0.39),
            (7_800, 0.43),
            (8_800, 0.40),
        ]
        .into_iter()
        .map(|(audio_end_ms, score)| LocalSpeakerEvidence {
            audio_end_ms,
            classification: if score >= 0.55 {
                crate::speaker_verification::SessionSpeakerClassification::Target { score }
            } else {
                crate::speaker_verification::SessionSpeakerClassification::Uncertain { score }
            },
            stable_target: true,
        })
        .collect();

        let target_text = "开始录音。";
        assert!(sequential_speaker_split_gap_is_owner_safe(
            &state,
            &result,
            target_text,
        ));

        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        *asr.state.lock() = state;
        let (tx, mut rx) = oneshot::channel();
        asr.state.lock().final_tx = Some(tx);
        let final_payload = serde_json::to_vec(&json!({
            "audio_info": { "duration": 10_322 },
            "result": result,
        }))
        .expect("session 621 final serializes");
        let final_frame = frame::build(
            MessageType::FullServerResponse,
            Flags::LastPacket,
            Serialization::Json,
            &final_payload,
            None,
        );

        assert!(!asr.handle_frame(&final_frame));
        let transcript = rx
            .try_recv()
            .expect("session 621 final should resolve")
            .expect("one owner split across three cloud ids should remain complete");
        assert_eq!(transcript.text, full);
    }

    #[test]
    fn owner_acceptance_final_recovers_locally_verified_unsegmented_tail() {
        // Owner acceptance 2026-08-22: all 7.36 s of PCM arrived, the provider
        // text reached the complete sentence, but final utterances stopped at
        // “苹果香蕉必须保留”. Diarization also split the same enrolled owner
        // across three sequential speaker ids. The final-only recovery must
        // keep the locally covered suffix without weakening live room-speech
        // isolation.
        let full = "开始录音嗯啊苹果香蕉必须保留然后继续说完整的后半句";
        let result = json!({
            "text": full,
            "utterances": [
                {
                    "additions": { "speaker_id": "0", "source": "two_pass" },
                    "definite": true,
                    "start_time": 80,
                    "end_time": 520,
                    "text": "开始录音"
                },
                {
                    "additions": { "speaker_id": "1", "source": "two_pass" },
                    "definite": true,
                    "start_time": 560,
                    "end_time": 962,
                    "text": "嗯啊"
                },
                {
                    "additions": { "speaker_id": "2", "source": "two_pass" },
                    "definite": true,
                    "start_time": 962,
                    "end_time": 3_822,
                    "text": "苹果香蕉必须保留"
                }
            ]
        });
        let evidence = [1_200, 1_800, 2_600, 3_400, 4_600, 5_400, 6_200]
            .into_iter()
            .map(|audio_end_ms| LocalSpeakerEvidence {
                audio_end_ms,
                classification:
                    crate::speaker_verification::SessionSpeakerClassification::Uncertain {
                        score: 0.42,
                    },
                stable_target: true,
            })
            .collect::<Vec<_>>();
        let mut state = SyncState {
            local_speaker_tracking_enabled: true,
            local_wake_owner_verified: true,
            local_speaker_profile_adaptive: false,
            local_speaker_stable_target: true,
            local_target_confirmed: true,
            local_speaker_classification: Some(
                crate::speaker_verification::SessionSpeakerClassification::Uncertain {
                    score: 0.42,
                },
            ),
            local_speaker_evidence: evidence,
            local_speech_end_ms: Some(6_500),
            target_speaker_id: Some("0".into()),
            wake_speaker_phrase: Some("开始录音".into()),
            ..SyncState::default()
        };

        assert!(!sequential_speaker_split_gap_is_owner_safe(
            &state,
            &result,
            "开始录音",
        ));
        assert!(final_unsegmented_provider_tail_is_owner_safe(
            &state,
            &result,
            "开始录音",
        ));

        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        state.last_server_audio_duration_ms = Some(7_360);
        *asr.state.lock() = state;
        let (tx, mut rx) = oneshot::channel();
        asr.state.lock().final_tx = Some(tx);
        let payload = serde_json::to_vec(&json!({
            "audio_info": { "duration": 7_360 },
            "result": result.clone(),
        }))
        .expect("owner acceptance final serializes");
        let frame = frame::build(
            MessageType::FullServerResponse,
            Flags::LastPacket,
            Serialization::Json,
            &payload,
            None,
        );

        assert!(!asr.handle_frame(&frame));
        let transcript = rx
            .try_recv()
            .expect("owner acceptance final should resolve")
            .expect("locally verified tail should remain successful");
        assert_eq!(transcript.text, full);

        let mut rejected = asr.state.lock();
        rejected.local_speaker_evidence.push(LocalSpeakerEvidence {
            audio_end_ms: 5_800,
            classification: crate::speaker_verification::SessionSpeakerClassification::NonTarget {
                score: 0.08,
            },
            stable_target: true,
        });
        assert!(!final_unsegmented_provider_tail_is_owner_safe(
            &rejected,
            &result,
            "开始录音",
        ));
    }

    #[test]
    fn installed_session_239_previews_split_without_uncertain_endpoint_refresh() {
        // The provider already had the body while the capsule remained blank:
        // wake phrase was stable speaker 0, continuous owner body was stable
        // speaker 1, and cross-phrase enrolled scores were Uncertain while the
        // debounced identity never left the owner. The identical evidence was
        // accepted only at protocol final, causing a ~5.9 s first preview.
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        asr.note_local_speaker_tracking_started("开始录音");

        let wake_payload = serde_json::to_vec(&json!({
            "audio_info": { "duration": 1_400 },
            "result": {
                "text": "开始录音。",
                "utterances": [{
                    "additions": { "speaker_id": "0", "source": "two_pass" },
                    "definite": true,
                    "start_time": 40,
                    "end_time": 1_002,
                    "text": "开始录音。"
                }]
            }
        }))
        .expect("wake packet serializes");
        let wake_frame = frame::build(
            MessageType::FullServerResponse,
            Flags::None,
            Serialization::Json,
            &wake_payload,
            None,
        );
        assert!(asr.handle_frame(&wake_frame));

        for (audio_end_ms, score) in [
            (2_200, 0.545_882_17),
            (3_200, 0.478_489_1),
            (4_200, 0.412_334_65),
        ] {
            asr.note_local_speaker_classification(
                audio_end_ms,
                crate::speaker_verification::SessionSpeakerClassification::Uncertain { score },
            );
        }

        let previews = Arc::new(ParkingMutex::new(Vec::new()));
        let callback_previews = Arc::clone(&previews);
        asr.set_partial_transcript_callback(Some(Arc::new(move |text| {
            callback_previews.lock().push(text);
        })));
        let endpoint_updates = Arc::new(ParkingMutex::new(Vec::new()));
        let callback_endpoint_updates = Arc::clone(&endpoint_updates);
        asr.set_target_speaker_update_callback(Some(Arc::new(move |update| {
            callback_endpoint_updates.lock().push(update);
        })));
        let expected = "开始录音。窗口不要报错，预览文字要及时稳定出现。";
        let body_payload = serde_json::to_vec(&json!({
            "audio_info": { "duration": 4_600 },
            "result": {
                "text": expected,
                "utterances": [
                    {
                        "additions": { "speaker_id": "0", "source": "two_pass" },
                        "definite": true,
                        "start_time": 40,
                        "end_time": 1_002,
                        "text": "开始录音。"
                    },
                    {
                        "additions": { "speaker_id": "1", "source": "two_pass" },
                        "definite": true,
                        "start_time": 1_120,
                        "end_time": 4_200,
                        "text": "窗口不要报错，预览文字要及时稳定出现。"
                    }
                ]
            }
        }))
        .expect("session 239 preview serializes");
        let body_frame = frame::build(
            MessageType::FullServerResponse,
            Flags::None,
            Serialization::Json,
            &body_payload,
            None,
        );
        assert!(asr.handle_frame(&body_frame));
        assert_eq!(previews.lock().last().map(String::as_str), Some(expected));
        let updates = endpoint_updates.lock();
        assert!(
            updates
                .iter()
                .any(|update| !update.pending_unattributed_speech),
            "corroborated owner text must not keep the endpoint pending"
        );
        assert!(
            updates
                .iter()
                .all(|update| update.local_target_speech_end_ms.is_none()),
            "Uncertain split preview may render but must not extend the owner endpoint"
        );
        drop(updates);

        // A later positive owner window restores endpoint authority without
        // taking away the low-latency split preview above.
        asr.note_local_speaker_classification(
            5_200,
            crate::speaker_verification::SessionSpeakerClassification::Target { score: 0.62 },
        );
        endpoint_updates.lock().clear();
        let continued = "开始录音。窗口不要报错，预览文字要及时稳定出现，然后继续。";
        let continued_payload = serde_json::to_vec(&json!({
            "audio_info": { "duration": 5_600 },
            "result": {
                "text": continued,
                "utterances": [
                    {
                        "additions": { "speaker_id": "0", "source": "two_pass" },
                        "definite": true,
                        "start_time": 40,
                        "end_time": 1_002,
                        "text": "开始录音。"
                    },
                    {
                        "additions": { "speaker_id": "1", "source": "two_pass" },
                        "definite": true,
                        "start_time": 1_120,
                        "end_time": 5_200,
                        "text": "窗口不要报错，预览文字要及时稳定出现，然后继续。"
                    }
                ]
            }
        }))
        .expect("session 239 continued preview serializes");
        let continued_frame = frame::build(
            MessageType::FullServerResponse,
            Flags::None,
            Serialization::Json,
            &continued_payload,
            None,
        );
        assert!(asr.handle_frame(&continued_frame));
        assert_eq!(previews.lock().last().map(String::as_str), Some(continued));
        assert!(
            endpoint_updates
                .lock()
                .iter()
                .any(|update| update.local_target_speech_end_ms.is_some()),
            "a current Target sample should restore owner endpoint refresh"
        );
    }

    #[test]
    fn adaptive_split_recovery_rejects_real_or_overlapping_other_speaker() {
        let sequential = json!({
            "text": "开始录音。主讲人第一句保持完整。旁边的人不应该被写进去。",
            "utterances": [
                {
                    "additions": { "speaker_id": "0", "source": "two_pass" },
                    "definite": true,
                    "start_time": 40,
                    "end_time": 4200,
                    "text": "开始录音。主讲人第一句保持完整。"
                },
                {
                    "additions": { "speaker_id": "1", "source": "two_pass" },
                    "definite": true,
                    "start_time": 4400,
                    "end_time": 6800,
                    "text": "旁边的人不应该被写进去。"
                }
            ]
        });
        let target_text = sequential["utterances"][0]["text"]
            .as_str()
            .expect("target utterance text")
            .to_string();
        let mut state = SyncState::default();
        state.local_speaker_tracking_enabled = true;
        state.local_speaker_profile_adaptive = true;
        state.local_speaker_stable_target = true;
        state.target_speaker_id = Some("0".into());
        state.local_speaker_evidence.push(LocalSpeakerEvidence {
            audio_end_ms: 5_600,
            classification: crate::speaker_verification::SessionSpeakerClassification::NonTarget {
                score: 0.18,
            },
            stable_target: true,
        });
        assert!(
            !sequential_speaker_split_gap_is_owner_safe(&state, &sequential, &target_text,),
            "local NonTarget evidence must keep a real second speaker excluded"
        );

        state.local_speaker_evidence[0].classification =
            crate::speaker_verification::SessionSpeakerClassification::Uncertain { score: 0.4 };
        state.local_non_target_speech_end_ms = Some(5_600);
        assert!(
            !sequential_speaker_split_gap_is_owner_safe(&state, &sequential, &target_text,),
            "confirmed adaptive NonTarget speech must reject provider-wide tail recovery"
        );
        state.local_non_target_speech_end_ms = None;
        let mut overlapping = sequential;
        overlapping["utterances"][1]["start_time"] = json!(3900);
        assert!(
            !sequential_speaker_split_gap_is_owner_safe(&state, &overlapping, &target_text,),
            "overlapping cloud speakers must never be merged into the owner transcript"
        );
    }

    #[test]
    fn installed_session_1705_allows_only_small_verified_owner_handoff_overlap() {
        let mut result = json!({
            "text": "开始录音。现在还能顺利地唤醒吗？自动结束还是一般般。",
            "utterances": [
                {
                    "additions": { "speaker_id": "1", "source": "two_pass" },
                    "definite": true,
                    "start_time": 40,
                    "end_time": 6982,
                    "text": "开始录音。现在还能顺利地唤醒吗？"
                },
                {
                    "additions": { "speaker_id": "2", "source": "two_pass" },
                    "definite": true,
                    // Two-pass boundaries may overlap slightly even though the
                    // physical speaker paused before continuing.
                    "start_time": 6860,
                    "end_time": 10192,
                    "text": "自动结束还是一般般。"
                }
            ]
        });
        let target_text = result["utterances"][0]["text"]
            .as_str()
            .expect("target utterance text")
            .to_string();
        let mut state = SyncState {
            local_speaker_tracking_enabled: true,
            local_wake_owner_verified: true,
            local_speaker_profile_adaptive: false,
            local_speaker_stable_target: true,
            local_target_confirmed: true,
            target_speaker_id: Some("1".into()),
            wake_speaker_phrase: Some("开始录音".into()),
            ..Default::default()
        };
        state.local_speaker_evidence = [7_500, 7_900, 8_300, 8_700, 9_100, 9_500, 9_900, 10_300]
            .into_iter()
            .map(|audio_end_ms| LocalSpeakerEvidence {
                audio_end_ms,
                classification:
                    crate::speaker_verification::SessionSpeakerClassification::Uncertain {
                        score: 0.31,
                    },
                stable_target: true,
            })
            .collect();

        assert!(sequential_speaker_split_gap_is_owner_safe(
            &state,
            &result,
            &target_text,
        ));

        state.local_wake_owner_verified = false;
        assert!(
            !sequential_speaker_split_gap_is_owner_safe(&state, &result, &target_text),
            "an unverified/adaptive profile must not weaken overlap isolation"
        );
        state.local_wake_owner_verified = true;
        result["utterances"][1]["start_time"] = json!(6680);
        assert!(
            !sequential_speaker_split_gap_is_owner_safe(&state, &result, &target_text),
            "a 302 ms overlap remains simultaneous speech and must be rejected"
        );
        result["utterances"][1]["start_time"] = json!(6860);
        state.local_speaker_classification = Some(
            crate::speaker_verification::SessionSpeakerClassification::NonTarget { score: 0.18 },
        );
        assert!(
            !sequential_speaker_split_gap_is_owner_safe(&state, &result, &target_text),
            "confirmed local non-target evidence still vetoes a small cloud overlap"
        );
    }

    #[test]
    fn session_52_final_packet_keeps_previously_attributed_body() {
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        asr.note_local_speaker_tracking_started("开始录音");
        for (audio_end_ms, score) in [
            (3_600, 0.413_043_53),
            (4_000, 0.402_944_15),
            (4_500, 0.445_347_25),
            (4_900, 0.501_113_24),
        ] {
            asr.note_local_speaker_classification(
                audio_end_ms,
                crate::speaker_verification::SessionSpeakerClassification::Uncertain { score },
            );
        }
        {
            let mut state = asr.state.lock();
            state.target_speaker_id = Some("0".into());
            state.stable_attributed_speech_end_ms = Some(5_872);
            state.last_server_audio_duration_ms = Some(8_460);
        }
        let (tx, mut rx) = oneshot::channel();
        asr.state.lock().final_tx = Some(tx);

        let final_payload = serde_json::to_vec(&json!({
            "audio_info": { "duration": 8_460 },
            "result": {
                "text": "开始录音。最终修复后的正文必须完全保留，并且录音能够正常结束。",
                "utterances": [{
                    "additions": { "speaker_id": "0", "source": "two_pass" },
                    "definite": true,
                    "start_time": 132,
                    "end_time": 1_552,
                    "text": "开始录音。"
                }]
            }
        }))
        .expect("session 52 final serializes");
        let final_frame = frame::build(
            MessageType::FullServerResponse,
            Flags::LastPacket,
            Serialization::Json,
            &final_payload,
            None,
        );

        assert!(!asr.handle_frame(&final_frame));
        let transcript = rx
            .try_recv()
            .expect("session 52 final should resolve")
            .expect("previously attributed owner body should remain successful");
        assert_eq!(
            transcript.text,
            "开始录音。最终修复后的正文必须完全保留，并且录音能够正常结束。"
        );
    }

    #[test]
    fn local_target_timeline_rescues_same_person_split_across_cloud_speakers() {
        let result = json!({
            "text": "开始录音本人继续说完整正文",
            "utterances": [
                {
                    "additions": { "speaker_id": "0", "source": "two_pass" },
                    "definite": true,
                    "start_time": 0,
                    "end_time": 1042,
                    "text": "开始录音"
                },
                {
                    "additions": { "speaker_id": "1", "source": "two_pass" },
                    "definite": true,
                    "start_time": 1200,
                    "end_time": 7652,
                    "text": "本人继续说完整正文"
                }
            ]
        });
        let evidence = vec![
            LocalSpeakerEvidence {
                audio_end_ms: 2_200,
                classification: crate::speaker_verification::SessionSpeakerClassification::Target {
                    score: 0.44,
                },
                stable_target: true,
            },
            LocalSpeakerEvidence {
                audio_end_ms: 3_100,
                classification: crate::speaker_verification::SessionSpeakerClassification::Target {
                    score: 0.43,
                },
                stable_target: true,
            },
            LocalSpeakerEvidence {
                audio_end_ms: 4_000,
                classification:
                    crate::speaker_verification::SessionSpeakerClassification::NonTarget {
                        score: 0.30,
                    },
                stable_target: true,
            },
            LocalSpeakerEvidence {
                audio_end_ms: 4_900,
                classification:
                    crate::speaker_verification::SessionSpeakerClassification::NonTarget {
                        score: 0.33,
                    },
                stable_target: false,
            },
            LocalSpeakerEvidence {
                audio_end_ms: 5_800,
                classification: crate::speaker_verification::SessionSpeakerClassification::Target {
                    score: 0.46,
                },
                stable_target: true,
            },
            LocalSpeakerEvidence {
                audio_end_ms: 6_700,
                classification: crate::speaker_verification::SessionSpeakerClassification::Target {
                    score: 0.45,
                },
                stable_target: true,
            },
        ];
        let mut target = None;

        let filtered = filter_result_to_target_speaker_with_local_evidence(
            &result,
            &mut target,
            true,
            &evidence,
            Some("开始录音"),
        );

        assert_eq!(target.as_deref(), Some("0"));
        assert_eq!(filtered.result["text"], "开始录音本人继续说完整正文");
        assert_eq!(
            filtered.optimistic_result["text"],
            "开始录音本人继续说完整正文"
        );
        assert_eq!(filtered.target_speech_end_ms, Some(7652));
    }

    #[test]
    fn local_non_target_timeline_rejects_real_other_cloud_speaker() {
        let result = json!({
            "text": "开始录音旁人正文",
            "utterances": [
                {
                    "additions": { "speaker_id": "0", "source": "two_pass" },
                    "definite": true,
                    "start_time": 0,
                    "end_time": 1042,
                    "text": "开始录音"
                },
                {
                    "additions": { "speaker_id": "1", "source": "two_pass" },
                    "definite": true,
                    "start_time": 1400,
                    "end_time": 5000,
                    "text": "旁人正文"
                }
            ]
        });
        let evidence = vec![
            LocalSpeakerEvidence {
                audio_end_ms: 2_400,
                classification:
                    crate::speaker_verification::SessionSpeakerClassification::NonTarget {
                        score: 0.20,
                    },
                stable_target: false,
            },
            LocalSpeakerEvidence {
                audio_end_ms: 3_200,
                classification:
                    crate::speaker_verification::SessionSpeakerClassification::NonTarget {
                        score: 0.18,
                    },
                stable_target: false,
            },
        ];
        let mut target = None;

        let filtered = filter_result_to_target_speaker_with_local_evidence(
            &result,
            &mut target,
            true,
            &evidence,
            Some("开始录音"),
        );

        assert_eq!(target.as_deref(), Some("0"));
        assert_eq!(filtered.result["text"], "开始录音");
        assert_eq!(filtered.target_speech_end_ms, Some(1042));
    }

    #[test]
    fn local_timeline_excludes_other_person_when_cloud_collapses_speaker_ids() {
        let result = json!({
            "text": "开始录音旁人正文",
            "utterances": [
                {
                    "additions": { "speaker_id": "0", "source": "two_pass" },
                    "definite": true,
                    "start_time": 0,
                    "end_time": 1200,
                    "text": "开始录音"
                },
                {
                    "additions": { "speaker_id": "0", "source": "two_pass" },
                    "definite": true,
                    "start_time": 1400,
                    "end_time": 4000,
                    "text": "旁人正文"
                }
            ]
        });
        let evidence = vec![
            LocalSpeakerEvidence {
                audio_end_ms: 1_000,
                classification: crate::speaker_verification::SessionSpeakerClassification::Target {
                    score: 0.7,
                },
                stable_target: true,
            },
            LocalSpeakerEvidence {
                audio_end_ms: 2_400,
                classification:
                    crate::speaker_verification::SessionSpeakerClassification::NonTarget {
                        score: 0.2,
                    },
                stable_target: false,
            },
            LocalSpeakerEvidence {
                audio_end_ms: 3_000,
                classification:
                    crate::speaker_verification::SessionSpeakerClassification::NonTarget {
                        score: 0.2,
                    },
                stable_target: false,
            },
        ];
        let mut target = None;

        let filtered = filter_result_to_target_speaker_with_local_evidence(
            &result,
            &mut target,
            true,
            &evidence,
            Some("开始录音"),
        );

        assert_eq!(target.as_deref(), Some("0"));
        assert_eq!(filtered.result["text"], "开始录音");
        assert_eq!(filtered.target_speech_end_ms, Some(1200));
    }

    #[test]
    fn long_cloud_utterance_containing_wake_phrase_still_requires_local_target_evidence() {
        let result = json!({
            "text": "开始录音旁人长句",
            "utterances": [{
                "additions": { "speaker_id": "0", "source": "two_pass" },
                "definite": true,
                "start_time": 0,
                "end_time": 4000,
                "text": "开始录音旁人长句"
            }]
        });
        let evidence = vec![
            LocalSpeakerEvidence {
                audio_end_ms: 2_400,
                classification:
                    crate::speaker_verification::SessionSpeakerClassification::NonTarget {
                        score: 0.2,
                    },
                stable_target: false,
            },
            LocalSpeakerEvidence {
                audio_end_ms: 3_000,
                classification:
                    crate::speaker_verification::SessionSpeakerClassification::NonTarget {
                        score: 0.2,
                    },
                stable_target: false,
            },
        ];
        let mut target = None;

        let filtered = filter_result_to_target_speaker_with_local_evidence(
            &result,
            &mut target,
            true,
            &evidence,
            Some("开始录音"),
        );

        assert_eq!(target.as_deref(), Some("0"));
        assert_eq!(filtered.result["text"], "");
        assert_eq!(filtered.target_speech_end_ms, None);
    }

    #[test]
    fn word_timing_recovers_missing_utterance_start_for_local_target_evidence() {
        let result = json!({
            "text": "开始录音本人正文",
            "utterances": [{
                "additions": { "speaker_id": "0", "source": "two_pass" },
                "definite": true,
                "end_time": 5400,
                "text": "开始录音本人正文",
                "words": [
                    { "start_time": 100, "end_time": 500, "text": "开始录音" },
                    { "start_time": 1500, "end_time": 5400, "text": "本人正文" }
                ]
            }]
        });
        let evidence = vec![LocalSpeakerEvidence {
            audio_end_ms: 2_100,
            classification: crate::speaker_verification::SessionSpeakerClassification::Target {
                score: 0.6,
            },
            stable_target: true,
        }];
        let mut target = None;

        let filtered = filter_result_to_target_speaker_with_local_evidence(
            &result,
            &mut target,
            true,
            &evidence,
            Some("开始录音"),
        );

        assert_eq!(target.as_deref(), Some("0"));
        assert_eq!(filtered.result["text"], "开始录音本人正文");
        assert_eq!(filtered.target_speech_end_ms, Some(5400));
    }

    #[test]
    fn other_speaker_does_not_advance_target_speech_time() {
        let result = json!({
            "text": "旁人一直说",
            "utterances": [{
                "additions": { "speaker": "2", "source": "stream" },
                "definite": true,
                "start_time": 2000,
                "end_time": 4200,
                "text": "旁人一直说"
            }]
        });
        let mut target = Some("1".to_string());

        let filtered = filter_result_to_target_speaker(&result, &mut target);

        assert!(filtered.speaker_info_present);
        assert_eq!(filtered.result["text"], "");
        assert!(filtered.result["utterances"].as_array().unwrap().is_empty());
        assert_eq!(filtered.target_speech_end_ms, None);
        assert_eq!(filtered.stable_attributed_speech_end_ms, Some(4200));
        assert!(filtered.pending_unattributed_text.is_empty());
    }

    #[test]
    fn local_speech_activity_is_provisional_until_stable_speaker_attribution_arrives() {
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        let updates = Arc::new(ParkingMutex::new(Vec::new()));
        let updates_for_callback = Arc::clone(&updates);
        asr.set_target_speaker_update_callback(Some(Arc::new(move |update| {
            updates_for_callback.lock().push(update);
        })));

        let stable_target_payload = serde_json::to_vec(&json!({
            "audio_info": { "duration": 2000 },
            "result": {
                "text": "第一句",
                "utterances": [{
                    "additions": { "speaker_id": "0", "source": "two_pass" },
                    "definite": true,
                    "start_time": 200,
                    "end_time": 1500,
                    "text": "第一句"
                }]
            }
        }))
        .expect("stable target response serializes");
        let stable_target_frame = frame::build(
            MessageType::FullServerResponse,
            Flags::None,
            Serialization::Json,
            &stable_target_payload,
            None,
        );
        assert!(asr.handle_frame(&stable_target_frame));

        asr.note_local_audio_activity(2400, true);
        asr.note_local_audio_activity(2500, false);
        let provisional = updates.lock().last().cloned().expect("local update");
        assert_eq!(provisional.target_speech_end_ms, Some(1500));
        assert_eq!(provisional.stable_attributed_speech_end_ms, Some(1500));
        assert_eq!(provisional.local_speech_end_ms, Some(2400));
        assert_eq!(provisional.audio_duration_ms, Some(2500));

        let stable_other_payload = serde_json::to_vec(&json!({
            "audio_info": { "duration": 2800 },
            "result": {
                "text": "第一句旁人",
                "utterances": [
                    {
                        "additions": { "speaker_id": "0", "source": "two_pass" },
                        "definite": true,
                        "start_time": 200,
                        "end_time": 1500,
                        "text": "第一句"
                    },
                    {
                        "additions": { "speaker_id": "1", "source": "two_pass" },
                        "definite": true,
                        "start_time": 1800,
                        "end_time": 2400,
                        "text": "旁人"
                    }
                ]
            }
        }))
        .expect("stable other response serializes");
        let stable_other_frame = frame::build(
            MessageType::FullServerResponse,
            Flags::None,
            Serialization::Json,
            &stable_other_payload,
            None,
        );
        assert!(asr.handle_frame(&stable_other_frame));
        let resolved = updates.lock().last().cloned().expect("resolved update");
        assert_eq!(resolved.target_speech_end_ms, Some(1500));
        assert_eq!(resolved.stable_attributed_speech_end_ms, Some(2400));
    }

    #[test]
    fn final_frame_send_budget_stays_below_final_result_wait() {
        assert!(WEBSOCKET_SEND_TIMEOUT < FINAL_RESULT_TIMEOUT);
        assert!(FINAL_FRAME_SEND_BUDGET < FINAL_RESULT_TIMEOUT);
    }

    #[test]
    fn retained_audio_survives_a_live_delivery_failure_until_finalization() {
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        let first = vec![1u8; 3_200];
        let tail = vec![2u8; 640];

        asr.consume_pcm_chunk(&first);
        asr.mark_audio_delivery_failed(VolcengineASRError::ConnectionFailed(
            "simulated send timeout".into(),
        ));
        asr.consume_pcm_chunk(&tail);

        let retained = asr.retained_pcm.lock();
        assert_eq!(retained.len(), first.len() + tail.len());
        assert_eq!(&retained[..first.len()], first.as_slice());
        assert_eq!(&retained[first.len()..], tail.as_slice());
    }

    #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
    #[tokio::test]
    async fn retained_audio_recovery_is_strictly_one_shot() {
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );

        let first = asr.replay_retained_audio_once().await.unwrap_err();
        assert!(first.to_string().contains("no retained PCM"));
        let second = asr.replay_retained_audio_once().await.unwrap_err();
        assert!(second.to_string().contains("already attempted"));
    }

    #[test]
    fn empty_final_replay_gain_is_bounded_and_never_amplifies_silence_or_hot_pcm() {
        let samples_to_pcm = |samples: &[i16]| {
            samples
                .iter()
                .flat_map(|sample| sample.to_le_bytes())
                .collect::<Vec<_>>()
        };

        let silence = samples_to_pcm(&[0, 0, 0, 0]);
        let (silence_out, silence_gain, _, _) = bounded_empty_final_replay_pcm(&silence);
        assert_eq!(silence_out, silence);
        assert_eq!(silence_gain, 1.0);

        let quiet = samples_to_pcm(&[100, -100, 400, -400]);
        let (_, quiet_gain, quiet_peak, quiet_after) = bounded_empty_final_replay_pcm(&quiet);
        assert!((quiet_gain - EMPTY_FINAL_REPLAY_MAX_GAIN).abs() < 0.000_001);
        assert_eq!(quiet_peak, 400);
        assert_eq!(quiet_after, 1_127);
        assert!(f64::from(quiet_after) <= EMPTY_FINAL_REPLAY_PEAK_CEILING.ceil());

        let hot = samples_to_pcm(&[30_000, -30_000]);
        let (hot_out, hot_gain, hot_peak, hot_after) = bounded_empty_final_replay_pcm(&hot);
        assert_eq!(hot_out, hot);
        assert_eq!(hot_gain, 1.0);
        assert_eq!(hot_peak, 30_000);
        assert_eq!(hot_after, 30_000);
    }

    #[test]
    fn uncertain_band_does_not_refresh_owner_endpoint_clock() {
        // F3 fixture（2026-08-09 12:47:04 弱 Target 序列 0.426–0.58）：置信
        // 余量带判 Uncertain 后不刷新本人端点时钟、也不冻结稳定身份；只有
        // 确信 Target 恢复刷新。
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        {
            let mut state = asr.state.lock();
            state.local_speaker_stable_target = true;
            state.local_target_confirmed = true;
        }
        use crate::speaker_verification::SessionSpeakerClassification as Classification;

        asr.note_local_speaker_classification(1_000, Classification::Target { score: 0.60 });
        assert_eq!(asr.state.lock().local_target_speech_end_ms, Some(1_000));

        // 复现得分序列（弱 Target 带 → Uncertain）：时钟不得推进。
        for (audio_end_ms, score) in [(1_400, 0.426), (2_200, 0.47), (3_000, 0.54)] {
            asr.note_local_speaker_classification(
                audio_end_ms,
                Classification::Uncertain { score },
            );
        }
        assert_eq!(asr.state.lock().local_target_speech_end_ms, Some(1_000));

        // 确信 Target 恢复刷新；稳定身份未被 Uncertain 序列冻结。
        asr.note_local_speaker_classification(3_600, Classification::Target { score: 0.58 });
        assert_eq!(asr.state.lock().local_target_speech_end_ms, Some(3_600));
        assert!(asr.state.lock().local_speaker_stable_target);
    }

    #[test]
    fn moderate_same_owner_negative_pair_remains_advisory() {
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        asr.note_verified_local_speaker_tracking_started("开始录音");
        {
            let mut state = asr.state.lock();
            state.local_speaker_stable_target = true;
            state.local_target_confirmed = true;
            state.best_transcript_text = "本人完整正文".into();
        }
        use crate::speaker_verification::SessionSpeakerClassification as Classification;
        asr.note_local_speaker_classification(4_000, Classification::NonTarget { score: 0.174 });
        asr.note_local_speaker_classification(4_400, Classification::NonTarget { score: 0.172 });

        let state = asr.state.lock();
        assert!(state.local_speaker_stable_target);
        assert!(!state.owner_isolation_frozen);
        assert_eq!(state.best_transcript_text, "本人完整正文");
    }

    #[test]
    fn local_shadow_recovery_requires_verified_owner_and_zero_other_speaker_evidence() {
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        assert!(!asr.may_run_local_shadow_decode());
        assert!(!asr.permits_local_shadow_omission_recovery());

        asr.note_verified_local_speaker_tracking_started("开始录音");
        assert!(asr.may_run_local_shadow_decode());
        assert!(!asr.permits_local_shadow_omission_recovery());
        {
            let mut state = asr.state.lock();
            state.local_target_confirmed = true;
            state.target_speech_end_ms = Some(7_912);
            state.local_target_speech_end_ms = Some(7_700);
        }
        assert!(asr.permits_local_shadow_omission_recovery());

        asr.state.lock().local_target_speech_end_ms = Some(6_900);
        assert!(!asr.permits_local_shadow_omission_recovery());
        asr.state.lock().local_target_speech_end_ms = Some(7_700);
        asr.state.lock().local_non_target_speech_end_ms = Some(4_000);
        assert!(!asr.permits_local_shadow_omission_recovery());
        asr.state.lock().local_non_target_speech_end_ms = None;
        asr.state.lock().owner_isolation_frozen = true;
        assert!(!asr.permits_local_shadow_omission_recovery());
    }

    #[test]
    fn extreme_local_pair_waits_for_provider_boundary_without_erasing_owner_growth() {
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        asr.note_verified_local_speaker_tracking_started("开始录音");
        {
            let mut state = asr.state.lock();
            state.local_speaker_stable_target = true;
            state.local_target_confirmed = true;
            state.best_transcript_text = "本人说完了".into();
            state.optimistic_preview_text = "本人说完了".into();
        }
        use crate::speaker_verification::SessionSpeakerClassification as Classification;
        asr.note_local_speaker_classification(5_950, Classification::NonTarget { score: -0.029 });
        {
            // Reproduce the destructive race: owner text advances after the
            // first hard mismatch but before the second verifier window. A
            // mixed owner+room window can produce exactly this score shape, so
            // local evidence alone must not roll the transcript backward.
            let mut state = asr.state.lock();
            state.best_transcript_text = "本人说完了旁人第一句".into();
            state.optimistic_preview_text = "本人说完了旁人第一句".into();
            state.last_partial_text = "本人说完了旁人第一句".into();
            state.last_emitted_preview_text = "本人说完了旁人第一句".into();
        }
        asr.note_local_speaker_classification(6_350, Classification::NonTarget { score: 0.077 });

        let state = asr.state.lock();
        assert!(state.local_speaker_stable_target);
        assert!(!state.owner_isolation_frozen);
        assert!(state.owner_isolation_ceiling_text.is_empty());
        assert_eq!(state.local_consecutive_transcript_hard_non_target, 2);
        assert_eq!(state.best_transcript_text, "本人说完了旁人第一句");
        assert_eq!(state.optimistic_preview_text, "本人说完了旁人第一句");
        assert_eq!(state.last_partial_text, "本人说完了旁人第一句");
        assert_eq!(state.last_emitted_preview_text, "本人说完了旁人第一句");
    }

    #[test]
    fn isolated_extreme_mismatch_discards_staged_checkpoint_without_rollback() {
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        asr.note_verified_local_speaker_tracking_started("开始录音");
        {
            let mut state = asr.state.lock();
            state.local_speaker_stable_target = true;
            state.local_target_confirmed = true;
            state.best_transcript_text = "本人第一句".into();
            state.optimistic_preview_text = "本人第一句".into();
        }
        use crate::speaker_verification::SessionSpeakerClassification as Classification;
        asr.note_local_speaker_classification(4_000, Classification::NonTarget { score: 0.05 });
        {
            let mut state = asr.state.lock();
            state.best_transcript_text = "本人第一句本人第二句".into();
            state.optimistic_preview_text = "本人第一句本人第二句".into();
        }
        asr.note_local_speaker_classification(4_400, Classification::Uncertain { score: 0.35 });

        let state = asr.state.lock();
        assert!(!state.owner_isolation_frozen);
        assert_eq!(state.local_consecutive_transcript_hard_non_target, 0);
        assert!(state.owner_isolation_ceiling_text.is_empty());
        assert_eq!(state.best_transcript_text, "本人第一句本人第二句");
    }

    #[test]
    fn short_hard_mismatch_pair_is_non_destructive_without_provider_boundary() {
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        asr.note_verified_local_speaker_tracking_started("开始录音");
        {
            let mut state = asr.state.lock();
            state.local_speaker_stable_target = true;
            state.local_target_confirmed = true;
            state.best_transcript_text = "只保留本人正文".into();
            state.optimistic_preview_text = "只保留本人正文".into();
        }
        use crate::speaker_verification::SessionSpeakerClassification as Classification;
        asr.note_local_speaker_observation(
            5_950,
            Classification::Uncertain { score: -0.029 },
            true,
        );
        assert!(!asr.state.lock().owner_isolation_frozen);
        asr.note_local_speaker_observation(6_350, Classification::Uncertain { score: 0.077 }, true);

        let state = asr.state.lock();
        assert!(state.local_speaker_stable_target);
        assert!(!state.owner_isolation_frozen);
        assert!(state.owner_isolation_ceiling_text.is_empty());
        assert_eq!(state.best_transcript_text, "只保留本人正文");
        assert_eq!(state.local_consecutive_transcript_hard_non_target, 2);
        assert_eq!(state.local_consecutive_non_target, 0);
        assert_eq!(state.local_consecutive_strong_non_target, 0);
        assert!(!state.local_owner_absence_run_confirmed);
        assert_eq!(state.local_non_target_speech_end_ms, None);
        assert_eq!(state.local_sustained_non_target_speech_end_ms, None);
    }

    #[test]
    fn empty_spin_retry_requires_sustained_local_speech_evidence() {
        // F2 fixture（2026-08-09 12:47:04）：云端空转会话音频 8957-10210ms、
        // 人声持续到尾端。仅唤醒词残段 / 音频太短 / 无本地证据不得触发重试。
        let new_asr = || {
            VolcengineStreamingASR::new(
                VolcengineCredentials {
                    app_id: "app".into(),
                    access_token: "token".into(),
                    resource_id: VolcengineCredentials::default_resource_id().into(),
                },
                Vec::new(),
            )
        };

        // 复现值域：整段人声（尾端距音频末尾 < 500ms slack）→ 允许重试。
        let sustained = new_asr();
        {
            let mut state = sustained.state.lock();
            state.local_audio_duration_ms = Some(10_210);
            state.local_speech_end_ms = Some(9_710);
        }
        assert!(sustained.has_sustained_local_speech_evidence());

        // 边界：恰好 500ms slack。
        let boundary = new_asr();
        {
            let mut state = boundary.state.lock();
            state.local_audio_duration_ms = Some(8_957);
            state.local_speech_end_ms = Some(8_457);
        }
        assert!(boundary.has_sustained_local_speech_evidence());

        // 仅唤醒词残段：人声停在 1845ms，音频近 9s → 不重试（正常空识别）。
        let wake_only = new_asr();
        {
            let mut state = wake_only.state.lock();
            state.local_audio_duration_ms = Some(8_957);
            state.local_speech_end_ms = Some(1_845);
        }
        assert!(!wake_only.has_sustained_local_speech_evidence());

        // 音频太短 / 无本地证据 → 不重试。
        let too_short = new_asr();
        {
            let mut state = too_short.state.lock();
            state.local_audio_duration_ms = Some(1_200);
            state.local_speech_end_ms = Some(1_200);
        }
        assert!(!too_short.has_sustained_local_speech_evidence());
        assert!(!new_asr().has_sustained_local_speech_evidence());
    }

    #[test]
    fn manual_short_empty_retry_requires_real_local_speech_and_enough_audio() {
        let new_asr = || {
            VolcengineStreamingASR::new(
                VolcengineCredentials {
                    app_id: "app".into(),
                    access_token: "token".into(),
                    resource_id: VolcengineCredentials::default_resource_id().into(),
                },
                Vec::new(),
            )
        };

        let short_speech = new_asr();
        {
            let mut state = short_speech.state.lock();
            state.local_audio_duration_ms = Some(2_120);
            state.local_speech_end_ms = Some(400);
        }
        assert!(short_speech.has_local_speech_evidence());
        assert!(!short_speech.has_sustained_local_speech_evidence());

        let silence = new_asr();
        silence.state.lock().local_audio_duration_ms = Some(2_120);
        assert!(!silence.has_local_speech_evidence());

        let too_short = new_asr();
        {
            let mut state = too_short.state.lock();
            state.local_audio_duration_ms = Some(1_200);
            state.local_speech_end_ms = Some(400);
        }
        assert!(!too_short.has_local_speech_evidence());
    }

    #[test]
    fn recovery_snapshot_preserves_local_wake_speaker_evidence() {
        let source = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        {
            let mut state = source.state.lock();
            state.local_audio_duration_ms = Some(10_600);
            state.local_target_speech_end_ms = Some(9_000);
            state.local_non_target_speech_end_ms = Some(10_600);
            state.local_speaker_tracking_enabled = true;
            state.local_speaker_stable_target = false;
            state.local_target_confirmed = true;
            state.local_consecutive_non_target = 2;
            state.local_consecutive_transcript_hard_non_target = 1;
            state.wake_speaker_phrase = Some("开始录音".into());
            state.owner_isolation_ceiling_text = "本人恢复检查点".into();
            state.local_speaker_evidence.push(LocalSpeakerEvidence {
                audio_end_ms: 10_600,
                classification:
                    crate::speaker_verification::SessionSpeakerClassification::NonTarget {
                        score: 0.2,
                    },
                stable_target: false,
            });
        }
        let replay = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );

        replay.restore_recovery_speaker_snapshot(source.recovery_speaker_snapshot());

        let state = replay.state.lock();
        assert!(state.local_speaker_tracking_enabled);
        assert!(!state.local_speaker_stable_target);
        assert!(state.local_target_confirmed);
        assert_eq!(state.local_target_speech_end_ms, Some(9_000));
        assert_eq!(state.local_non_target_speech_end_ms, Some(10_600));
        assert_eq!(state.local_consecutive_non_target, 2);
        assert_eq!(state.local_consecutive_transcript_hard_non_target, 1);
        assert_eq!(state.local_speaker_evidence.len(), 1);
        assert_eq!(state.wake_speaker_phrase.as_deref(), Some("开始录音"));
        assert_eq!(state.owner_isolation_ceiling_text, "本人恢复检查点");
    }

    #[test]
    fn only_transport_and_incomplete_final_errors_permit_full_audio_replay() {
        assert!(VolcengineASRError::ConnectionFailed("timeout".into()).permits_full_audio_replay());
        assert!(VolcengineASRError::NoFinalResult.permits_full_audio_replay());
        assert!(VolcengineASRError::FinalResultTimeout.permits_full_audio_replay());
        assert!(!VolcengineASRError::CredentialsMissing.permits_full_audio_replay());
        assert!(!VolcengineASRError::AuthRejected(401).permits_full_audio_replay());
        assert!(
            !VolcengineASRError::QuotaExceeded(VOLCENGINE_AUDIO_DURATION_QUOTA_EXCEEDED)
                .permits_full_audio_replay()
        );
        assert!(!VolcengineASRError::DecodeFailed("bad frame".into()).permits_full_audio_replay());
    }

    #[test]
    fn provider_quota_exhaustion_is_actionable_and_not_a_transport_failure() {
        let exact = classify_provider_error(
            VOLCENGINE_AUDIO_DURATION_QUOTA_EXCEEDED,
            "quota exceeded for types: audio_duration_lifetime",
        );
        assert!(matches!(
            exact,
            VolcengineASRError::QuotaExceeded(VOLCENGINE_AUDIO_DURATION_QUOTA_EXCEEDED)
        ));
        assert!(exact.to_string().contains("补充 ASR 时长"));
        assert!(!exact.permits_full_audio_replay());

        let future_code = classify_provider_error(45_999_999, "Quota Exceeded");
        assert!(matches!(
            future_code,
            VolcengineASRError::QuotaExceeded(45_999_999)
        ));

        let transport = classify_provider_error(45_000_001, "temporary server error");
        assert!(matches!(transport, VolcengineASRError::ConnectionFailed(_)));
        assert!(transport.permits_full_audio_replay());
    }

    #[tokio::test]
    async fn send_binary_times_out_when_writer_lock_is_stuck() {
        let writer: SharedWriter = Arc::new(AsyncMutex::new(None));
        let _guard = writer.lock().await;
        let start = Instant::now();

        let err = send_binary_with_timeout(&writer, vec![1, 2, 3], Duration::from_millis(10))
            .await
            .unwrap_err();

        assert!(
            start.elapsed() < Duration::from_millis(500),
            "send should fail on the short timeout"
        );
        assert!(
            err.to_string().contains("writer lock timed out"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn audio_delivery_readiness_waits_for_bridge_flush() {
        let asr = Arc::new(VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        ));
        let ready_asr = Arc::clone(&asr);
        let ready = tokio::spawn(async move {
            tokio::task::yield_now().await;
            ready_asr.mark_audio_delivery_ready();
        });

        asr.await_audio_delivery_ready(Duration::from_millis(100))
            .await
            .expect("bridge readiness should release the final-frame waiter");
        ready.await.expect("readiness task should finish");
    }

    #[tokio::test]
    async fn audio_delivery_readiness_returns_the_real_startup_failure() {
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        asr.mark_audio_delivery_failed(VolcengineASRError::ConnectionFailed(
            "TLS handshake rejected".into(),
        ));

        let error = asr
            .await_audio_delivery_ready(Duration::from_millis(10))
            .await
            .expect_err("a failed opener must not become websocket-not-open");
        assert!(error.to_string().contains("TLS handshake rejected"));
    }

    #[tokio::test]
    async fn deferred_open_failure_waits_until_recovery_pcm_is_sealed() {
        let asr = Arc::new(VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: String::new(),
                access_token: String::new(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        ));

        let open_error = asr
            .open_session_for_deferred_audio()
            .await
            .expect_err("missing credentials deterministically fail startup");
        assert!(matches!(
            asr.state.lock().audio_delivery_readiness,
            AudioDeliveryReadiness::Opening
        ));

        let buffered_prefix = vec![7u8; 3_200];
        asr.consume_pcm_chunk(&buffered_prefix);
        assert_eq!(
            asr.retained_pcm.lock().as_slice(),
            buffered_prefix.as_slice()
        );

        asr.mark_audio_delivery_failed(open_error);
        let readiness_error = asr
            .await_audio_delivery_ready(Duration::from_millis(10))
            .await
            .expect_err("failure is published only after recovery PCM is retained");
        assert!(matches!(
            readiness_error,
            VolcengineASRError::CredentialsMissing
        ));
    }

    #[tokio::test]
    async fn twenty_deferred_open_failures_retain_every_pcm_byte_before_publication() {
        for attempt in 0u8..20 {
            let asr = Arc::new(VolcengineStreamingASR::new(
                VolcengineCredentials {
                    app_id: String::new(),
                    access_token: String::new(),
                    resource_id: VolcengineCredentials::default_resource_id().into(),
                },
                Vec::new(),
            ));
            let error = asr
                .open_session_for_deferred_audio()
                .await
                .expect_err("empty credentials inject a deterministic open failure");
            let pcm = vec![attempt.saturating_add(1); 3_200 + usize::from(attempt) * 2];
            asr.consume_pcm_chunk(&pcm);
            assert_eq!(asr.retained_pcm.lock().as_slice(), pcm.as_slice());
            assert!(matches!(
                asr.state.lock().audio_delivery_readiness,
                AudioDeliveryReadiness::Opening
            ));
            asr.mark_audio_delivery_failed(error);
            assert!(asr
                .await_audio_delivery_ready(Duration::from_millis(10))
                .await
                .is_err());
            assert_eq!(asr.retained_pcm.lock().as_slice(), pcm.as_slice());
        }
    }

    #[test]
    fn twenty_send_failures_keep_prefix_and_tail_losslessly() {
        for attempt in 0u8..20 {
            let asr = VolcengineStreamingASR::new(
                VolcengineCredentials {
                    app_id: "app".into(),
                    access_token: "token".into(),
                    resource_id: VolcengineCredentials::default_resource_id().into(),
                },
                Vec::new(),
            );
            let prefix = vec![attempt.saturating_add(1); 3_200];
            let tail = vec![attempt.saturating_add(21); 640];
            asr.consume_pcm_chunk(&prefix);
            asr.mark_audio_delivery_failed(VolcengineASRError::ConnectionFailed(format!(
                "injected send failure {attempt}"
            )));
            asr.consume_pcm_chunk(&tail);
            let retained = asr.retained_pcm.lock();
            assert_eq!(retained.len(), prefix.len() + tail.len());
            assert_eq!(&retained[..prefix.len()], prefix.as_slice());
            assert_eq!(&retained[prefix.len()..], tail.as_slice());
            assert!(!asr.recovery_replay_started.load(Ordering::SeqCst));
        }
    }

    #[test]
    fn twenty_final_failures_allow_exactly_one_complete_replay_claim() {
        for attempt in 0u8..20 {
            let asr = VolcengineStreamingASR::new(
                VolcengineCredentials {
                    app_id: "app".into(),
                    access_token: "token".into(),
                    resource_id: VolcengineCredentials::default_resource_id().into(),
                },
                Vec::new(),
            );
            let pcm = vec![attempt.saturating_add(1); 6_400 + usize::from(attempt) * 2];
            asr.consume_pcm_chunk(&pcm);
            asr.mark_audio_delivery_failed(VolcengineASRError::NoFinalResult);
            let (claimed, _) = asr
                .claim_retained_audio_for_replay()
                .expect("first final-failure recovery claim succeeds");
            assert_eq!(claimed, pcm);
            let duplicate = match asr.claim_retained_audio_for_replay() {
                Ok(_) => panic!("a second replay claim must be rejected"),
                Err(error) => error,
            };
            assert!(duplicate.to_string().contains("already attempted"));
        }
    }

    #[test]
    fn final_audio_drain_budget_scales_with_queued_audio() {
        assert_eq!(final_audio_drain_budget(0), FINAL_AUDIO_DRAIN_MIN_BUDGET);
        assert_eq!(final_audio_drain_budget(3), FINAL_AUDIO_DRAIN_MIN_BUDGET);
        assert_eq!(final_audio_drain_budget(27), Duration::from_millis(2_700));
        assert_eq!(final_audio_drain_budget(200), FINAL_AUDIO_DRAIN_MAX_BUDGET);
    }

    #[tokio::test]
    async fn audio_delivery_drain_returns_the_real_worker_failure() {
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        asr.mark_audio_delivery_ready();
        asr.mark_audio_delivery_failed(VolcengineASRError::ConnectionFailed(
            "websocket send timed out after 1200 ms".into(),
        ));

        let error = asr
            .await_audio_delivery_drained(Duration::from_millis(10))
            .await
            .expect_err("a failed queued send must stop the final-frame path");
        assert!(error
            .to_string()
            .contains("websocket send timed out after 1200 ms"));
    }

    #[tokio::test]
    async fn audio_delivery_drain_waits_for_queued_audio_before_finalization() {
        let asr = Arc::new(VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        ));
        asr.mark_audio_delivery_ready();
        asr.pending_sends.store(1, Ordering::SeqCst);

        let drained_asr = Arc::clone(&asr);
        let drain = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            assert_eq!(drained_asr.pending_sends.fetch_sub(1, Ordering::SeqCst), 1);
            drained_asr.send_done.notify_waiters();
        });

        let started = Instant::now();
        asr.await_audio_delivery_drained(Duration::from_millis(100))
            .await
            .expect("the final frame must wait for the queued audio worker");
        assert!(started.elapsed() >= Duration::from_millis(10));
        drain.await.expect("audio worker drain task should finish");
    }

    #[tokio::test]
    async fn await_final_result_returns_error_when_final_frame_never_arrives() {
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        let (tx, rx) = oneshot::channel();
        asr.state.lock().final_tx = Some(tx);
        *asr.final_rx.lock() = Some(rx);

        let result = asr
            .await_final_result_with_timeout(std::time::Duration::from_millis(10))
            .await;

        assert!(matches!(
            result,
            Err(VolcengineASRError::FinalResultTimeout)
        ));
    }

    #[tokio::test]
    async fn proactive_final_frame_is_idempotent_and_seals_later_audio() {
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        asr.mark_audio_delivery_ready();
        {
            let mut state = asr.state.lock();
            state.is_connected = true;
            state.next_sequence = 7;
        }

        let first = asr
            .send_last_frame()
            .await
            .expect_err("test writer is intentionally absent");
        let sequence_after_first = asr.state.lock().next_sequence;
        let second = asr
            .send_last_frame()
            .await
            .expect_err("the cached final-frame result should be reused");
        assert_eq!(first.to_string(), second.to_string());
        assert_eq!(asr.state.lock().next_sequence, sequence_after_first);
        assert!(asr.state.lock().finishing);

        asr.consume_pcm_chunk(&vec![1; TARGET_AUDIO_CHUNK_BYTES]);
        let state = asr.state.lock();
        assert!(state.pending_audio.is_empty());
        assert_eq!(state.frames_sent, 0);
    }

    #[tokio::test]
    async fn await_final_result_rejects_stable_partial_without_protocol_final() {
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        let (tx, rx) = oneshot::channel();
        {
            let mut state = asr.state.lock();
            state.final_tx = Some(tx);
            state.finishing = true;
            state.start = Some(Instant::now());
            state.best_transcript_text = "帮我录音。怎么退？".into();
            state.last_partial_text = state.best_transcript_text.clone();
        }
        *asr.final_rx.lock() = Some(rx);

        let result = asr
            .await_final_result_with_timeout(std::time::Duration::from_millis(10))
            .await;

        assert!(matches!(
            result,
            Err(VolcengineASRError::FinalResultTimeout)
        ));
    }

    #[tokio::test]
    async fn await_final_result_rejects_uncovered_stable_partial() {
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        let (tx, rx) = oneshot::channel();
        {
            let mut state = asr.state.lock();
            state.final_tx = Some(tx);
            state.finishing = true;
            state.start = Some(Instant::now());
            state.bytes_sent = 138_040;
            state.best_transcript_text = "帮我录音。".into();
            state.best_transcript_segments = vec![TranscriptSegment {
                start_ms: 800,
                end_ms: Some(2_202),
                text: "帮我录音。".into(),
            }];
        }
        *asr.final_rx.lock() = Some(rx);

        assert_eq!(asr.final_partial_coverage_gap(), Some((4_313, 2_202)));
        let result = asr
            .await_final_result_with_timeout(std::time::Duration::from_millis(10))
            .await;

        assert!(matches!(
            result,
            Err(VolcengineASRError::FinalResultCoverageIncomplete {
                sent_audio_ms: 4_313,
                transcript_end_ms: 2_202,
            })
        ));
    }

    #[tokio::test]
    async fn await_final_result_requires_protocol_final_even_with_full_server_audio() {
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        let (tx, rx) = oneshot::channel();
        {
            let mut state = asr.state.lock();
            state.final_tx = Some(tx);
            state.finishing = true;
            state.start = Some(Instant::now());
            state.bytes_sent = 138_040;
            state.last_server_audio_duration_ms = Some(4_313);
            state.best_transcript_text = "帮我录音。怎么退？".into();
            state.best_transcript_segments = vec![TranscriptSegment {
                start_ms: 800,
                end_ms: Some(2_692),
                text: "帮我录音。".into(),
            }];
        }
        *asr.final_rx.lock() = Some(rx);

        assert_eq!(asr.final_partial_coverage_gap(), None);
        let result = asr
            .await_final_result_with_timeout(std::time::Duration::from_millis(10))
            .await;

        assert!(matches!(
            result,
            Err(VolcengineASRError::FinalResultTimeout)
        ));
    }

    #[derive(Default)]
    struct ProviderCadenceCallbackStats {
        callbacks: u64,
        first_callback_ms: Option<u64>,
        previous_callback_ms: Option<u64>,
        max_callback_gap_ms: u64,
    }

    #[derive(serde::Serialize)]
    struct ProviderCadenceProbeVariant {
        endpoint: String,
        enable_nonstream: bool,
        result_type: String,
        callback_count: u64,
        first_callback_ms: Option<u64>,
        max_callback_gap_ms: Option<u64>,
        final_chars: Option<usize>,
        final_text: Option<String>,
        final_elapsed_ms: Option<u64>,
        error_kind: Option<String>,
    }

    #[derive(serde::Serialize)]
    struct ProviderCadenceProbeSummary {
        mode: &'static str,
        input_sha256: String,
        input_duration_ms: u64,
        variants: Vec<ProviderCadenceProbeVariant>,
    }

    fn record_provider_cadence_callback(
        stats: &ParkingMutex<ProviderCadenceCallbackStats>,
        elapsed_ms: u64,
    ) {
        let mut stats = stats.lock();
        stats.callbacks += 1;
        stats.first_callback_ms.get_or_insert(elapsed_ms);
        if let Some(previous_ms) = stats.previous_callback_ms {
            stats.max_callback_gap_ms = stats
                .max_callback_gap_ms
                .max(elapsed_ms.saturating_sub(previous_ms));
        }
        stats.previous_callback_ms = Some(elapsed_ms);
    }

    async fn run_live_provider_cadence_variant(
        credentials: VolcengineCredentials,
        pcm: &[u8],
        options: VolcengineSessionOptions,
    ) -> ProviderCadenceProbeVariant {
        let endpoint = options.endpoint.label().to_string();
        let enable_nonstream = options.enable_nonstream;
        let result_type = options.result_type.as_str().to_string();
        let asr = Arc::new(VolcengineStreamingASR::new_with_session_options(
            credentials,
            Vec::new(),
            options,
        ));
        let started = Instant::now();
        let stats = Arc::new(ParkingMutex::new(ProviderCadenceCallbackStats::default()));
        let partial_stats = Arc::clone(&stats);
        let partial_started = started;
        asr.set_partial_transcript_callback(Some(Arc::new(move |_| {
            record_provider_cadence_callback(
                &partial_stats,
                partial_started.elapsed().as_millis() as u64,
            );
        })));
        let supplement_stats = Arc::clone(&stats);
        let supplement_started = started;
        asr.set_final_intermediate_transcript_callback(Some(Arc::new(move |_| {
            record_provider_cadence_callback(
                &supplement_stats,
                supplement_started.elapsed().as_millis() as u64,
            );
        })));

        let run = async {
            asr.open_session().await?;
            for chunk in pcm.chunks(TARGET_AUDIO_CHUNK_BYTES) {
                asr.consume_pcm_chunk(chunk);
                tokio::time::sleep(Duration::from_millis(
                    (chunk.len() as f64 / BYTES_PER_MS).round() as u64,
                ))
                .await;
            }
            // The provider may finalize a speech-complete stream before the
            // harness reaches its explicit stop. Keep reading the final result
            // instead of misclassifying that close as an endpoint failure.
            let finish_result = asr.send_last_frame().await;
            match asr
                .await_final_result_with_timeout(Duration::from_secs(15))
                .await
            {
                Ok(final_result) => Ok(final_result),
                Err(final_error) => match finish_result {
                    Ok(()) => Err(final_error),
                    Err(finish_error) => Err(VolcengineASRError::ConnectionFailed(format!(
                        "finish error: {finish_error}; final error: {final_error}"
                    ))),
                },
            }
        }
        .await;

        let stats = stats.lock();
        match run {
            Ok(final_result) => ProviderCadenceProbeVariant {
                endpoint,
                enable_nonstream,
                result_type,
                callback_count: stats.callbacks,
                first_callback_ms: stats.first_callback_ms,
                max_callback_gap_ms: (stats.callbacks > 1).then_some(stats.max_callback_gap_ms),
                final_chars: Some(final_result.text.chars().count()),
                final_text: Some(final_result.text),
                final_elapsed_ms: Some(started.elapsed().as_millis() as u64),
                error_kind: None,
            },
            Err(error) => ProviderCadenceProbeVariant {
                endpoint,
                enable_nonstream,
                result_type,
                callback_count: stats.callbacks,
                first_callback_ms: stats.first_callback_ms,
                max_callback_gap_ms: (stats.callbacks > 1).then_some(stats.max_callback_gap_ms),
                final_chars: None,
                final_text: None,
                final_elapsed_ms: Some(started.elapsed().as_millis() as u64),
                error_kind: Some(error.to_string()),
            },
        }
    }

    #[tokio::test]
    #[ignore = "requires explicit temporary synthetic PCM and installed Volcengine credentials"]
    async fn live_provider_cadence_probe_compares_authoritative_endpoints() {
        use crate::persistence::{CredentialAccount, CredentialsVault};
        use std::fs;
        use std::path::PathBuf;

        let pcm_path = std::env::var("LISTENER_PROVIDER_CADENCE_PCM_PATH")
            .expect("explicit temporary PCM input is required");
        let summary_path = std::env::var("LISTENER_PROVIDER_CADENCE_SUMMARY_PATH")
            .expect("explicit numeric summary path is required");
        let pcm = fs::read(&pcm_path).expect("read temporary PCM input");
        assert!(
            !pcm.is_empty() && pcm.len() % TARGET_AUDIO_CHUNK_BYTES == 0,
            "probe PCM must be a nonempty 16 kHz/16-bit/mono stream in 100 ms frames"
        );
        assert!(
            pcm.len() <= 60_000 * BYTES_PER_MS as usize,
            "probe PCM must not exceed 60,000 ms"
        );
        let input_sha256 = {
            use sha2::{Digest, Sha256};
            format!("{:x}", Sha256::digest(&pcm))
        };

        let credential = |account| {
            CredentialsVault::get(account)
                .expect("read installed Volcengine credential")
                .filter(|value| !value.trim().is_empty())
                .expect("installed Volcengine credential is required")
        };
        let credentials = VolcengineCredentials {
            app_id: credential(CredentialAccount::VolcengineAppKey),
            access_token: credential(CredentialAccount::VolcengineAccessKey),
            resource_id: credential(CredentialAccount::VolcengineResourceId),
        };

        let variants = [
            VolcengineSessionOptions::default(),
            VolcengineSessionOptions {
                endpoint: VolcengineSessionEndpoint::Bidirectional,
                enable_nonstream: false,
                result_type: VolcengineResultType::Full,
                end_window_size_ms: Some(SECOND_PASS_END_WINDOW_MS),
                force_to_speech_time_ms: Some(SECOND_PASS_FORCE_TO_SPEECH_MS),
            },
        ];
        let mut results = Vec::with_capacity(variants.len());
        for options in variants {
            results
                .push(run_live_provider_cadence_variant(credentials.clone(), &pcm, options).await);
        }

        let summary_path = PathBuf::from(summary_path);
        if let Some(parent) = summary_path.parent() {
            fs::create_dir_all(parent).expect("create provider cadence summary directory");
        }
        let all_succeeded = results.iter().all(|result| result.error_kind.is_none());
        fs::write(
            summary_path,
            serde_json::to_vec_pretty(&ProviderCadenceProbeSummary {
                mode: "live_provider_cadence_ab",
                input_sha256,
                input_duration_ms: (pcm.len() as f64 / BYTES_PER_MS) as u64,
                variants: results,
            })
            .expect("serialize provider cadence summary"),
        )
        .expect("write provider cadence summary");

        eprintln!(
            "provider_cadence_probe_status={}",
            if all_succeeded { "PASS" } else { "NO_GO" }
        );
    }

    /// Runs the same provider diarization and local session-voiceprint path as
    /// an installed automatic-wake session.  This is deliberately ignored:
    /// callers must supply consented/public 16 kHz PCM fixtures and installed
    /// provider credentials explicitly.  It exists to prevent overlap tests
    /// from accidentally measuring only the provider's unfiltered transcript.
    #[tokio::test]
    #[ignore = "requires explicit public owner/mix PCM and installed Volcengine credentials"]
    async fn live_target_speaker_filter_probe_uses_real_local_observations() {
        use crate::persistence::{CredentialAccount, CredentialsVault};
        use std::fs;
        use std::path::PathBuf;

        let owner_pcm_path = std::env::var("LISTENER_TARGET_OWNER_PCM_PATH")
            .expect("LISTENER_TARGET_OWNER_PCM_PATH is required");
        let mix_pcm_path = std::env::var("LISTENER_TARGET_MIX_PCM_PATH")
            .expect("LISTENER_TARGET_MIX_PCM_PATH is required");
        let summary_path = std::env::var("LISTENER_TARGET_FILTER_SUMMARY_PATH")
            .expect("LISTENER_TARGET_FILTER_SUMMARY_PATH is required");
        let expected_text = std::env::var("LISTENER_TARGET_EXPECTED_TEXT")
            .expect("LISTENER_TARGET_EXPECTED_TEXT is required");
        let filter_expected = std::env::var("LISTENER_TARGET_FILTER_EXPECTED")
            .map(|value| value == "1")
            .unwrap_or(true);
        let wake_phrase =
            std::env::var("LISTENER_TARGET_WAKE_PHRASE").unwrap_or_else(|_| "我们要做".to_string());
        let owner_pcm = fs::read(&owner_pcm_path).expect("read owner PCM");
        let mix_pcm = fs::read(&mix_pcm_path).expect("read overlap PCM");
        for (label, pcm) in [("owner", &owner_pcm), ("mix", &mix_pcm)] {
            assert!(
                !pcm.is_empty() && pcm.len() % 2 == 0,
                "{label} PCM must be nonempty 16-bit audio"
            );
            assert!(
                pcm.len() <= 60_000 * BYTES_PER_MS as usize,
                "{label} PCM must not exceed 60 seconds"
            );
        }

        let credential = |account| {
            CredentialsVault::get(account)
                .expect("read installed Volcengine credential")
                .filter(|value| !value.trim().is_empty())
                .expect("installed Volcengine credential is required")
        };
        let credentials = VolcengineCredentials {
            app_id: credential(CredentialAccount::VolcengineAppKey),
            access_token: credential(CredentialAccount::VolcengineAccessKey),
            resource_id: credential(CredentialAccount::VolcengineResourceId),
        };
        let owner_duration_seconds = owner_pcm.len() as f32 / 32_000.0;
        let profile_phrase = wake_phrase.clone();
        let owner_for_profile = owner_pcm.clone();
        let profile = tokio::task::spawn_blocking(move || {
            crate::speaker_verification::session_profile_from_wake(
                &owner_for_profile,
                owner_duration_seconds,
                &profile_phrase,
            )
        })
        .await
        .expect("join public owner profile task")
        .expect("build public owner session profile");
        // Match product startup ordering: KWS/speaker runtime first, target
        // extraction preload second, both before the wake-owned body starts.
        let warm_started = Instant::now();
        crate::asr::target_speaker_extraction::warm_up()
            .expect("prepare target-speaker model before paced capture");
        let warm_up_elapsed_ms = warm_started.elapsed().as_millis();

        let asr = Arc::new(VolcengineStreamingASR::new_with_session_options(
            credentials,
            Vec::new(),
            VolcengineSessionOptions::default(),
        ));
        // Model the installed path after the persisted owner gate accepted the
        // wake.  The public fixture profile itself is ephemeral, so explicitly
        // mark the ASR-side policy as verified/non-adaptive for this probe.
        asr.note_verified_local_speaker_tracking_started(&wake_phrase);
        asr.note_local_speaker_profile_adaptive(false);
        asr.start_target_speaker_extraction(&owner_pcm, owner_duration_seconds);
        asr.open_session().await.expect("open provider session");
        asr.mark_audio_delivery_ready();

        let mut rolling = Vec::<u8>::with_capacity((LOCAL_SPEAKER_WINDOW_MS * 32) as usize);
        let mut next_observation_ms = 1_000u64;
        let mut observations = Vec::<serde_json::Value>::new();
        let mut audio_end_ms = 0u64;
        for chunk in mix_pcm.chunks(TARGET_AUDIO_CHUNK_BYTES) {
            audio_end_ms = audio_end_ms.saturating_add((chunk.len() as u64) / 32);
            rolling.extend_from_slice(chunk);
            let window_bytes = (LOCAL_SPEAKER_WINDOW_MS * 32) as usize;
            if rolling.len() > window_bytes {
                let overflow = (rolling.len() - window_bytes + 1) & !1usize;
                rolling.drain(..overflow);
            }
            let has_signal = chunk
                .chunks_exact(2)
                .map(|sample| i16::from_le_bytes([sample[0], sample[1]]).unsigned_abs())
                .max()
                .unwrap_or_default()
                >= 512;
            asr.note_local_audio_activity(audio_end_ms, has_signal);
            if has_signal && audio_end_ms >= next_observation_ms && rolling.len() >= 1_000 * 32 {
                if let Ok(observation) =
                    crate::speaker_verification::observe_session_speaker(&profile, &rolling)
                {
                    observations.push(serde_json::json!({
                        "audioEndMs": audio_end_ms,
                        "score": observation.classification.score(),
                        "classification": format!("{:?}", observation.classification),
                        "transcriptHardNonTarget": observation.transcript_hard_non_target,
                    }));
                    asr.note_local_speaker_observation(
                        audio_end_ms,
                        observation.classification,
                        observation.transcript_hard_non_target,
                    );
                }
                next_observation_ms = audio_end_ms.saturating_add(400);
            }
            asr.consume_pcm_chunk(chunk);
            tokio::time::sleep(Duration::from_millis(
                (chunk.len() as f64 / BYTES_PER_MS).round().max(1.0) as u64,
            ))
            .await;
        }
        let finalization_started = Instant::now();
        asr.send_last_frame().await.expect("send final frame");
        let final_result = asr
            .await_final_result_with_timeout(Duration::from_secs(20))
            .await
            .expect("receive filtered final");
        let primary_final_elapsed_ms = finalization_started.elapsed().as_millis();
        let target_result = asr
            .await_target_speaker_final()
            .await
            .expect("receive target-speaker selection result");
        let target_final_elapsed_ms = finalization_started.elapsed().as_millis();
        let selected_text = target_result
            .as_ref()
            .map(|target| target.text.as_str())
            .unwrap_or(final_result.text.as_str())
            .to_string();
        let passed = selected_text.trim() == expected_text.trim()
            && target_result.is_some() == filter_expected;
        let state = asr.state.lock();
        let report = serde_json::json!({
            "status": if passed { "PASS" } else { "FAIL" },
            "wakePhrase": wake_phrase,
            "ownerProfileAdaptive": profile.is_adaptive(),
            "inputDurationMs": audio_end_ms,
            "filteredTranscript": final_result.text,
            "filteredChars": final_result.text.chars().count(),
            "expectedOwnerTranscript": expected_text,
            "targetFilterExpected": filter_expected,
            "targetFilterUsed": target_result.is_some(),
            "targetExtractedTranscript": target_result.as_ref().map(|target| target.text.as_str()),
            "targetExtractedChars": target_result.as_ref().map(|target| target.text.chars().count()),
            "selectedTranscript": selected_text,
            "primaryFinalElapsedMs": primary_final_elapsed_ms,
            "targetFinalElapsedMs": target_final_elapsed_ms,
            "modelWarmUpElapsedMs": warm_up_elapsed_ms,
            "targetSpeakerId": state.target_speaker_id,
            "speakerInfoPresent": state.speaker_info_present,
            "ownerIsolationFrozen": state.owner_isolation_frozen,
            "ownerIsolationCeilingChars": state.owner_isolation_ceiling_text.chars().count(),
            "observations": observations,
        });
        drop(state);
        let summary_path = PathBuf::from(summary_path);
        if let Some(parent) = summary_path.parent() {
            fs::create_dir_all(parent).expect("create target-filter summary directory");
        }
        fs::write(
            &summary_path,
            serde_json::to_vec_pretty(&report).expect("serialize target-filter report"),
        )
        .expect("write target-filter report");
        eprintln!(
            "target_speaker_filter_probe_report={}",
            summary_path.display()
        );
        assert!(passed, "target-speaker filter probe report is FAIL");
    }
}
