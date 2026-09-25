//! Volcengine SAUC bigmodel streaming ASR client.
//!
//! Direct port of the Swift `VolcengineStreamingASR`. Battle-tested protocol
//! quirks are preserved verbatim — see comments tagged with `[asr]` for the
//! original learnings (especially the "definite=true is NOT stream end" bug).

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex as ParkingMutex;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::runtime::Handle;
use tokio::sync::{mpsc, oneshot, Mutex as AsyncMutex, Notify, OnceCell};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::HeaderValue;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{
    client_async_tls_with_config, connect_async, MaybeTlsStream, WebSocketStream,
};
use uuid::Uuid;

use crate::polish::{EffectiveProxyMode, ProviderProxyConfig};

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
// 2026-09-21 跟手优化（stable-ledger early seal）：停说→上屏原本 = 端点耐心窗
// + 云端 two-pass 终稿往返。端点 STOP 本身证明最后一条账本变更之后全是静音
// （inactive 1s + 续说窗都无正向证据），所以账本在停止时刻已是"完整终稿"：
// 给真实终稿一个短先手，没到就从稳定账本直接封存，省掉 two-pass 往返。
// 仅干净会话启用（无隔离冻结、无非目标窗）；干扰会话仍等完整终稿走归属仲裁。
const EARLY_SEAL_FINAL_HEAD_START: Duration = Duration::from_millis(350);
const EARLY_SEAL_MIN_LEDGER_STABLE: Duration = Duration::from_millis(1_500);
// Physical separation is a fallback for ambiguous overlap, not a reason to
// hold an already-visible transcript for another twelve seconds. On the
// shipping CPU the separator can lag real time by several seconds per chunk;
// keep its post-stop contribution inside the product's processing budget.
// Installed r12 (first real-voice-bank session, 2026-09-18): the wait fired
// only after interference was positively detected (physical_overlap +
// sustained_non_target), and `finish()` still had to run one tail extraction
// (measured 1285-1940 ms) plus the secondary ASR last-frame/final round trip
// (~1-2 s). The old 1500 ms budget therefore guaranteed a timeout exactly
// when the owner-only final was required, sealing the masked primary instead
// (first-chunk swallowing). 6 s covers tail + one lagged 3 s chunk + the
// finalization round trip while staying half of FINAL_RESULT_TIMEOUT; clean
// sessions never reach this path (the required-gate returns fast).
const TARGET_SPEAKER_FINAL_WAIT: Duration = Duration::from_millis(6_000);
const PROVIDER_CLEAN_NON_TARGET_TAIL_GAP_MS: u64 = 600;
const FINAL_PARTIAL_COVERAGE_SLACK_MS: u64 = 600;
const WEBSOCKET_SEND_TIMEOUT: Duration = Duration::from_millis(2_500);
const DIAGNOSTIC_TRACE_MAX_ENTRIES: usize = 512;
const DIAGNOSTIC_TRACE_MAX_CHARS: usize = 512;
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
const PROXY_CONNECT_HEADER_LIMIT: usize = 16 * 1024;
const FINAL_FRAME_SEND_BUDGET: Duration = Duration::from_millis(1_800);
const FINAL_AUDIO_DRAIN_MIN_BUDGET: Duration = Duration::from_millis(800);
// A full-audio replay queues its captured frames at once. An eight-second cap
// rejected a 67-second recording with 192 frames still in the FIFO even while
// the socket was delivering audio. Bound the wait by queued audio duration.
const FINAL_AUDIO_DRAIN_MAX_BUDGET: Duration = Duration::from_secs(45);
const AUDIO_FRAME_DURATION_MS: u64 = 100;
static NEXT_VOLCENGINE_ASR_STREAM_ID: AtomicU64 = AtomicU64::new(1);

fn next_volcengine_asr_stream_id() -> u64 {
    NEXT_VOLCENGINE_ASR_STREAM_ID.fetch_add(1, Ordering::Relaxed)
}
// Keep streamed preview acceleration independent of second-pass segmentation.
// Aggressive 500 ms hard-VAD boundaries repeatedly returned empty completed
// clauses on full captured speech. A one-second boundary preserves those
// clauses without delaying the stream's first-character callbacks.
const SECOND_PASS_END_WINDOW_MS: u32 = 1_000;
// The provider documents a minimum of 1 and recommends 1000 ms. Zero is
// outside that contract and delayed initial callbacks in paced PCM probes.
const SECOND_PASS_FORCE_TO_SPEECH_MS: u32 = 1_000;

#[derive(Clone, Debug)]
pub struct VolcengineCredentials {
    pub app_id: String,
    pub access_token: String,
    pub resource_id: String,
}

impl VolcengineCredentials {
    pub fn default_resource_id() -> &'static str {
        // 2026-09-21: the gateway now rejects "volc.seedasr.sauc.duration" at
        // handshake for every caller (HTTP 400 "resourceId ... is not allowed",
        // verified with garbage credentials). "volc.bigasr.sauc.duration" is the
        // accepted id for the Doubao Seed-ASR streaming 2.0 hour pack and serves
        // the same /api/v3/sauc/bigmodel endpoint.
        "volc.bigasr.sauc.duration"
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

async fn connect_ws_with_network_policy(
    request: tokio_tungstenite::tungstenite::handshake::client::Request,
    proxy_config: &ProviderProxyConfig,
) -> Result<
    (
        WsStream,
        tokio_tungstenite::tungstenite::handshake::client::Response,
    ),
    tokio_tungstenite::tungstenite::Error,
> {
    let endpoint = request.uri().to_string();
    let mode = proxy_config.effective_mode(&endpoint);
    // 2026-09-22 45000081 根治：直连名单里的国内 ASR 供应商（火山语音）在
    // System 模式下会搭系统代理——本地代理（Clash 等）黑洞长连 WSS 时，
    // 云端 8 秒收不到包就杀会话（实测 2026-09-22 每 1-2 小时 1 单，
    // ~15-20% 会话：预览全死 + 断流重放兜底一次性蹦全文）。这些端点在
    // 国内本就不需要代理；显式 Custom 仍然尊重，System 视同 Direct。
    let mode = match mode {
        EffectiveProxyMode::System
            if crate::polish::provider_default_proxy_is_direct(proxy_config.provider_id()) =>
        {
            log::info!(
                "[network] domestic ASR provider bypasses system proxy provider={} (direct-default list)",
                proxy_config.provider_id()
            );
            EffectiveProxyMode::Direct
        }
        mode => mode,
    };
    let proxy_url = match mode {
        EffectiveProxyMode::Direct => None,
        EffectiveProxyMode::Custom => proxy_config.custom_proxy_url().map(str::to_owned),
        EffectiveProxyMode::System => system_http_proxy_url(&endpoint),
    };

    log::info!(
        "[network] Volcengine WebSocket policy provider={} mode={:?} proxy={}",
        proxy_config.provider_id(),
        mode,
        proxy_url.as_deref().unwrap_or("DIRECT")
    );

    let Some(proxy_url) = proxy_url else {
        return connect_async(request).await;
    };

    let proxy = url::Url::parse(&proxy_url).map_err(|error| {
        tokio_tungstenite::tungstenite::Error::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("invalid WebSocket proxy URL: {error}"),
        ))
    })?;
    if proxy.scheme() != "http" {
        return Err(tokio_tungstenite::tungstenite::Error::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "WebSocket proxy must use http:// (SOCKS/HTTPS proxy is not supported by this transport)",
        )));
    }
    let proxy_host = proxy.host_str().ok_or_else(|| {
        tokio_tungstenite::tungstenite::Error::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "WebSocket proxy host is missing",
        ))
    })?;
    let proxy_port = proxy.port().unwrap_or(80);
    let target_host = request.uri().host().ok_or_else(|| {
        tokio_tungstenite::tungstenite::Error::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "WebSocket target host is missing",
        ))
    })?;
    let target_port = request.uri().port_u16().unwrap_or_else(|| {
        if request.uri().scheme_str() == Some("wss") {
            443
        } else {
            80
        }
    });
    let mut stream = TcpStream::connect((proxy_host, proxy_port))
        .await
        .map_err(tokio_tungstenite::tungstenite::Error::Io)?;

    let authority = format!("{target_host}:{target_port}");
    let mut connect_request = format!(
        "CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\nProxy-Connection: Keep-Alive\r\n"
    );
    if !proxy.username().is_empty() {
        let userinfo = if let Some(password) = proxy.password() {
            format!("{}:{}", proxy.username(), password)
        } else {
            proxy.username().to_string()
        };
        use base64::Engine;
        let encoded = base64::engine::general_purpose::STANDARD.encode(userinfo);
        connect_request.push_str(&format!("Proxy-Authorization: Basic {encoded}\r\n"));
    }
    connect_request.push_str("\r\n");
    stream
        .write_all(connect_request.as_bytes())
        .await
        .map_err(tokio_tungstenite::tungstenite::Error::Io)?;

    let mut response = Vec::with_capacity(1024);
    let mut byte = [0_u8; 1];
    while response.len() < PROXY_CONNECT_HEADER_LIMIT {
        let read = stream
            .read(&mut byte)
            .await
            .map_err(tokio_tungstenite::tungstenite::Error::Io)?;
        if read == 0 {
            break;
        }
        response.push(byte[0]);
        if response.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let header = String::from_utf8_lossy(&response);
    let status_ok = header
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|status| status.parse::<u16>().ok())
        .is_some_and(|status| (200..300).contains(&status));
    if !status_ok {
        return Err(tokio_tungstenite::tungstenite::Error::Io(
            std::io::Error::new(
                std::io::ErrorKind::ConnectionRefused,
                format!(
                    "HTTP proxy CONNECT rejected: {}",
                    header.lines().next().unwrap_or("<empty>")
                ),
            ),
        ));
    }

    client_async_tls_with_config(request, stream, None, None).await
}

fn system_http_proxy_url(endpoint: &str) -> Option<String> {
    let target_host = url::Url::parse(endpoint)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned));
    if let Some(host) = target_host.as_deref() {
        let no_proxy = std::env::var("NO_PROXY")
            .ok()
            .or_else(|| std::env::var("no_proxy").ok());
        if no_proxy
            .as_deref()
            .is_some_and(|entries| no_proxy_matches_host(entries, host))
        {
            log::info!("[network] system proxy bypassed by NO_PROXY for host={host}");
            return None;
        }
    }
    for key in [
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
        "ALL_PROXY",
        "all_proxy",
    ] {
        if let Ok(value) = std::env::var(key) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    if let Ok(proxy) = sysproxy::Sysproxy::get_system_proxy() {
        if proxy.enable && !proxy.host.trim().is_empty() && proxy.port != 0 {
            return Some(format!("http://{}:{}", proxy.host.trim(), proxy.port));
        }
    }
    None
}

fn no_proxy_matches_host(entries: &str, host: &str) -> bool {
    entries.split(',').any(|entry| {
        let token = entry.trim().trim_start_matches('.');
        if token.is_empty() {
            return false;
        }
        if token == "*" {
            return true;
        }
        let token = token
            .rsplit_once(':')
            .map(|(host, port)| {
                if port.chars().all(|c| c.is_ascii_digit()) {
                    host
                } else {
                    token
                }
            })
            .unwrap_or(token);
        host == token || host.ends_with(&format!(".{token}"))
    })
}

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

#[derive(Clone, Debug, serde::Serialize)]
pub struct VolcengineDiagnosticTraceEntry {
    pub frame: usize,
    pub elapsed_ms: u128,
    pub final_frame: bool,
    pub stage: &'static str,
    pub text: String,
    pub text_truncated: bool,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct VolcengineDiagnosticTrace {
    pub entries: Vec<VolcengineDiagnosticTraceEntry>,
    pub entries_dropped: usize,
    pub unassociated_audio_frame_count: usize,
    pub unassociated_audio_pcm_bytes: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocalSpeakerClassificationKind {
    Target,
    NonTarget,
    Uncertain,
}

impl From<&crate::speaker_verification::SessionSpeakerClassification>
    for LocalSpeakerClassificationKind
{
    fn from(classification: &crate::speaker_verification::SessionSpeakerClassification) -> Self {
        match classification {
            crate::speaker_verification::SessionSpeakerClassification::Target { .. } => Self::Target,
            crate::speaker_verification::SessionSpeakerClassification::NonTarget { .. } => {
                Self::NonTarget
            }
            crate::speaker_verification::SessionSpeakerClassification::Uncertain { .. } => {
                Self::Uncertain
            }
        }
    }
}

/// Independent local human-voice evidence used by the host endpoint reducer.
///
/// This is deliberately separate from the enrolled-speaker classifier: VAD
/// answers only "is there human speech here?", while the classifier answers
/// "whose speech is it?".  A missing, lagging, or failed VAD result is kept
/// conservative by the endpoint adapter and must not be treated as silence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalSpeechActivityState {
    Speech,
    /// A separate fast-onset VAD has found a possible new utterance, but the
    /// canonical detector has not yet met its speech-duration confirmation
    /// window.  This is endpoint hold evidence only; it is not an owner
    /// watermark and must never authorize transcript or speaker output.
    PendingSpeech,
    NonSpeech,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalSpeechEvidence {
    pub analyzed_through_ms: u64,
    pub analyzed_through_samples: u64,
    pub last_detected_speech_end_ms: Option<u64>,
    pub pending_speech_start_ms: Option<u64>,
    pub trailing_non_speech_ms: Option<u64>,
    /// Increments when a new Speech/PendingSpeech interval begins. Quiet VAD
    /// revisions keep the same epoch; endpoint STOP admission uses this to
    /// distinguish harmless silence refinement from a new tail that must
    /// revoke a pending STOP proposal.
    pub activity_epoch: u64,
    pub revision: u64,
    pub state: LocalSpeechActivityState,
}

impl Default for LocalSpeechEvidence {
    fn default() -> Self {
        Self {
            analyzed_through_ms: 0,
            analyzed_through_samples: 0,
            last_detected_speech_end_ms: None,
            pending_speech_start_ms: None,
            trailing_non_speech_ms: None,
            activity_epoch: 0,
            revision: 0,
            state: LocalSpeechActivityState::Unknown,
        }
    }
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
    /// Raw low-level energy edge. This is diagnostic/hold evidence only; it
    /// must never be treated as a positive enrolled-owner watermark.
    pub local_speech_end_ms: Option<u64>,
    /// Positive owner activity observed by the local speaker classifier with
    /// usable signal quality. This is the only local watermark allowed to
    /// extend an enrolled-owner endpoint.
    pub qualified_owner_speech_end_ms: Option<u64>,
    /// True only on the update emitted for a newly classified, qualified
    /// owner interval. Provider/raw callbacks must leave this false.
    pub qualified_owner_activity_advanced: bool,
    /// Latest local speaker observation summary. The coverage end identifies
    /// which audio was actually classified; callers must not apply its result
    /// to newer raw audio merely because the retained identity is Target.
    pub local_speaker_classification_kind: Option<LocalSpeakerClassificationKind>,
    pub local_speaker_signal_quality_sufficient: Option<bool>,
    pub local_speaker_observation_end_ms: Option<u64>,
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
    pub accelerate_score: u8,
    pub result_type: VolcengineResultType,
    pub end_window_size_ms: Option<u32>,
    pub force_to_speech_time_ms: Option<u32>,
}

impl Default for VolcengineSessionOptions {
    fn default() -> Self {
        Self {
            endpoint: VolcengineSessionEndpoint::OptimizedBidirectional,
            enable_nonstream: true,
            // The provider defaults this to zero (no acceleration), even
            // when enable_accelerate_text is true. A recorded-device A/B at
            // 10 reduced the first callback from 9.7s to 3.4s with identical
            // final text; keep two-pass correction enabled.
            accelerate_score: 10,
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
    trim_repeated_short_final_tail, trim_repeated_short_streaming_tail, TranscriptCandidate,
    TranscriptSegment,
};
use super::volcengine_untimed_merge::{
    merge_filtered_streaming_candidate_with_untimed_window, merge_optimistic_cumulative_view,
};

/// Sync state shared across the receive loop, the public API, and the
/// audio-consumer fast path.
#[derive(Default)]
struct SyncState {
    #[cfg(test)]
    diagnostic_provider_responses: Vec<Value>,
    pending_audio: Vec<u8>,
    pending_audio_sources: VecDeque<AudioSourceRun>,
    next_input_destination_offset: u64,
    destination_ledger_incomplete: bool,
    pending_audio_destination_start: Option<u64>,
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
    /// When `best_transcript_text` last changed. Repeated identical cloud
    /// revisions must not refresh it: stability is the early-seal contract.
    best_transcript_committed_at: Option<Instant>,
    /// Recent owner-ledger revisions for rolling prefix stability. A growing
    /// tail must not reset the age of an unchanged earlier clause.
    pause_early_recent_revisions: VecDeque<(Instant, String)>,
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
    // A later cloud revision must not reclassify withheld room speech as an
    // unseen owner-recovery tail. Filtered owner rows may still grow normally.
    local_preview_exclusion_seen: bool,
    /// A signal-qualified mismatch is advisory, but a new unattributed row
    /// across that window must wait for identity before entering visible text.
    local_preview_foreign_hint_end_ms: Option<u64>,
    /// 最新服务端响应已处理的音频时长。two-pass 终帧可能只给最后一个 utterance 的
    /// 词时间戳，但 `audio_info.duration` 仍覆盖整段音频；收尾时应优先采用这项
    /// 传输级覆盖证据，避免把已返回的完整文本误判为截断。
    last_server_audio_duration_ms: Option<u64>,
    /// send_last_frame 时刻的云端覆盖水位（2026-09-23 18:49 会话 5ac70fc7
    /// 实锤：用户停顿 ~2.6s 后在 3s 窗口内续说，端点竞速输了照常 STOP，但
    /// 尾音经 drain 通路继续上云，终稿 58 字完整含续说——却被 STOP 天花板
    /// 砍回 45 字）。终稿覆盖显著超过此水位 = 尾巴有新捕获音频背书（真实
    /// 续说），而非两遍幻听；owner_preview_safety_ceiling 据此豁免。
    stop_boundary_server_audio_ms: Option<u64>,
    /// Most recent server response. This is an abort clock only until the
    /// first response; later gaps are observed but cannot prove a dead socket.
    last_server_response_at: Option<Instant>,
    /// 最近一次服务端应答之后累计入队的音频字节数（与 `bytes_sent` 同口径）。
    bytes_sent_since_response: usize,
    post_response_gap_logged: bool,
    target_speaker_id: Option<String>,
    target_speech_end_ms: Option<u64>,
    /// Stable end of the physical wake row. Unlike `target_speech_end_ms`,
    /// this never advances with dictation and can safely anchor an immediate
    /// provider speaker-cluster split across later streaming responses.
    wake_target_speech_end_ms: Option<u64>,
    stable_attributed_speech_end_ms: Option<u64>,
    local_audio_duration_ms: Option<u64>,
    local_speech_end_ms: Option<u64>,
    qualified_owner_speech_end_ms: Option<u64>,
    local_speaker_signal_quality_sufficient: Option<bool>,
    local_speaker_observation_end_ms: Option<u64>,
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
    /// Sticky, non-destructive evidence that two consecutive transcript-grade
    /// owner mismatches ended at this audio position. It never deletes already
    /// accepted owner text. It only vetoes fail-open provider-tail recovery
    /// when the provider's stable foreign-speaker interval overlaps the same
    /// physical audio (installed interference session 2744).
    local_confirmed_transcript_non_target_end_ms: Option<u64>,
    /// Two consecutive short but high-energy extreme mismatches. This weaker
    /// checkpoint is never passed to same-cluster filtering and cannot erase
    /// text alone; it only corroborates a stable provider foreign-speaker row.
    local_consecutive_transcript_foreign_hint: u8,
    local_confirmed_transcript_foreign_hint_end_ms: Option<u64>,
    /// A new provider utterance and a persisted-owner voiceprint mismatch have
    /// jointly proposed a speaker handoff. Keep this sticky across later cloud
    /// revisions: the provider may relabel the foreign row with the owner's old
    /// speaker id. While set, raw VAD, Uncertain windows and provider growth do
    /// not advance the owner endpoint. Two consecutive Target windows clear it.
    local_owner_handoff_suspected: bool,
    local_owner_handoff_recovery_targets: u8,
    /// Session voiceprint negatives are advisory for transcript ownership. Two
    /// very-low-score windows may still mark current speech as another person
    /// for endpointing, but must never freeze/delete provider-recognized text.
    local_consecutive_strong_non_target: u8,
    local_owner_absence_run_started_ms: Option<u64>,
    local_owner_absence_run_confirmed: bool,
    local_speaker_evidence: Vec<LocalSpeakerEvidence>,
    local_unreliable_speaker_evidence_end_ms: Vec<u64>,
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
    /// Final provider text attributed only to the wake-bound speaker when the
    /// same final also contains a distinct stable speaker. This is a safe
    /// fallback if physical separation drops a large owner tail.
    distinct_speaker_target_final_text: Option<String>,
    /// Final provider text from one stable speaker track, bound to the
    /// voiceprint-verified wake owner. Body voiceprint windows may remain
    /// uncertain (installed 61→15 and 37→20 sessions), so absence of explicit
    /// other-speaker evidence—not repeated per-sentence identity—is the safety
    /// condition for the noise-only separator fallback.
    wake_bound_single_speaker_final_text: Option<String>,
    /// Append-only raw provider evidence. Ownership/preview policies derive
    /// candidates from it but never rewrite it.
    transcript_evidence: crate::speech_decision_kernel::TranscriptEvidenceLedger,
}

#[derive(Clone, Debug)]
struct OwnerContinuitySnapshot {
    tracking_enabled: bool,
    wake_owner_verified: bool,
    profile_adaptive: bool,
    wake_phrase: Option<String>,
    // Opening the provider transport is not a new product recording session.
    // Local owner analysis can already have consumed the buffered wake/body
    // PCM while the WebSocket is connecting, so its complete continuity
    // state must cross the transport reset as one snapshot. Preserving only
    // the booleans creates an impossible split state (`stable_target=true`
    // with no confirmed owner watermark) and lets the endpoint stop while the
    // same owner is still speaking.
    local_audio_duration_ms: Option<u64>,
    local_speech_end_ms: Option<u64>,
    qualified_owner_speech_end_ms: Option<u64>,
    local_speaker_signal_quality_sufficient: Option<bool>,
    local_speaker_observation_end_ms: Option<u64>,
    local_target_speech_end_ms: Option<u64>,
    local_non_target_speech_end_ms: Option<u64>,
    local_sustained_non_target_speech_end_ms: Option<u64>,
    local_speaker_classification: Option<crate::speaker_verification::SessionSpeakerClassification>,
    stable_target: bool,
    target_confirmed: bool,
    consecutive_target: u8,
    consecutive_non_target: u8,
    consecutive_transcript_hard_non_target: u8,
    confirmed_transcript_non_target_end_ms: Option<u64>,
    consecutive_transcript_foreign_hint: u8,
    confirmed_transcript_foreign_hint_end_ms: Option<u64>,
    preview_foreign_hint_end_ms: Option<u64>,
    owner_handoff_suspected: bool,
    owner_handoff_recovery_targets: u8,
    consecutive_strong_non_target: u8,
    owner_absence_run_started_ms: Option<u64>,
    owner_absence_run_confirmed: bool,
    evidence: Vec<LocalSpeakerEvidence>,
    unreliable_evidence_end_ms: Vec<u64>,
}

impl OwnerContinuitySnapshot {
    fn capture(state: &SyncState) -> Self {
        Self {
            tracking_enabled: state.local_speaker_tracking_enabled,
            wake_owner_verified: state.local_wake_owner_verified,
            profile_adaptive: state.local_speaker_profile_adaptive,
            wake_phrase: state.wake_speaker_phrase.clone(),
            local_audio_duration_ms: state.local_audio_duration_ms,
            local_speech_end_ms: state.local_speech_end_ms,
            qualified_owner_speech_end_ms: state.qualified_owner_speech_end_ms,
            local_speaker_signal_quality_sufficient: state.local_speaker_signal_quality_sufficient,
            local_speaker_observation_end_ms: state.local_speaker_observation_end_ms,
            local_target_speech_end_ms: state.local_target_speech_end_ms,
            local_non_target_speech_end_ms: state.local_non_target_speech_end_ms,
            local_sustained_non_target_speech_end_ms: state
                .local_sustained_non_target_speech_end_ms,
            local_speaker_classification: state.local_speaker_classification.clone(),
            stable_target: state.local_speaker_stable_target,
            target_confirmed: state.local_target_confirmed,
            consecutive_target: state.local_consecutive_target,
            consecutive_non_target: state.local_consecutive_non_target,
            consecutive_transcript_hard_non_target: state
                .local_consecutive_transcript_hard_non_target,
            confirmed_transcript_non_target_end_ms: state
                .local_confirmed_transcript_non_target_end_ms,
            consecutive_transcript_foreign_hint: state.local_consecutive_transcript_foreign_hint,
            preview_foreign_hint_end_ms: state.local_preview_foreign_hint_end_ms,
            owner_handoff_suspected: state.local_owner_handoff_suspected,
            owner_handoff_recovery_targets: state.local_owner_handoff_recovery_targets,
            confirmed_transcript_foreign_hint_end_ms: state
                .local_confirmed_transcript_foreign_hint_end_ms,
            consecutive_strong_non_target: state.local_consecutive_strong_non_target,
            owner_absence_run_started_ms: state.local_owner_absence_run_started_ms,
            owner_absence_run_confirmed: state.local_owner_absence_run_confirmed,
            evidence: state.local_speaker_evidence.clone(),
            unreliable_evidence_end_ms: state.local_unreliable_speaker_evidence_end_ms.clone(),
        }
    }

    fn restore(self, state: &mut SyncState) {
        state.local_speaker_tracking_enabled = self.tracking_enabled;
        state.local_wake_owner_verified = self.tracking_enabled && self.wake_owner_verified;
        state.local_speaker_profile_adaptive = self.tracking_enabled && self.profile_adaptive;
        state.wake_speaker_phrase = self.wake_phrase;
        if !self.tracking_enabled {
            return;
        }
        state.local_audio_duration_ms = self.local_audio_duration_ms;
        state.local_speech_end_ms = self.local_speech_end_ms;
        state.qualified_owner_speech_end_ms = self.qualified_owner_speech_end_ms;
        state.local_speaker_signal_quality_sufficient =
            self.local_speaker_signal_quality_sufficient;
        state.local_speaker_observation_end_ms = self.local_speaker_observation_end_ms;
        state.local_target_speech_end_ms = self.local_target_speech_end_ms;
        state.local_non_target_speech_end_ms = self.local_non_target_speech_end_ms;
        state.local_sustained_non_target_speech_end_ms =
            self.local_sustained_non_target_speech_end_ms;
        state.local_speaker_classification = self.local_speaker_classification;
        state.local_speaker_stable_target = self.stable_target;
        state.local_target_confirmed = self.target_confirmed;
        state.local_consecutive_target = self.consecutive_target;
        state.local_consecutive_non_target = self.consecutive_non_target;
        state.local_consecutive_transcript_hard_non_target =
            self.consecutive_transcript_hard_non_target;
        state.local_confirmed_transcript_non_target_end_ms =
            self.confirmed_transcript_non_target_end_ms;
        state.local_consecutive_transcript_foreign_hint = self.consecutive_transcript_foreign_hint;
        state.local_preview_foreign_hint_end_ms = self.preview_foreign_hint_end_ms;
        state.local_owner_handoff_suspected = self.owner_handoff_suspected;
        state.local_owner_handoff_recovery_targets = self.owner_handoff_recovery_targets;
        state.local_confirmed_transcript_foreign_hint_end_ms =
            self.confirmed_transcript_foreign_hint_end_ms;
        state.local_consecutive_strong_non_target = self.consecutive_strong_non_target;
        state.local_owner_absence_run_started_ms = self.owner_absence_run_started_ms;
        state.local_owner_absence_run_confirmed = self.owner_absence_run_confirmed;
        state.local_speaker_evidence = self.evidence;
        state.local_unreliable_speaker_evidence_end_ms = self.unreliable_evidence_end_ms;
    }
}

#[derive(Clone, Copy, Debug)]
struct LocalSpeakerEvidence {
    audio_end_ms: u64,
    classification: crate::speaker_verification::SessionSpeakerClassification,
    stable_target: bool,
}

const LOCAL_SPEAKER_SWITCH_CONFIRMATIONS: u8 = 2;
const LOCAL_SPEAKER_WINDOW_MS: u64 = 1_200;
const LOCAL_ENDPOINT_STRONG_NON_TARGET_MAX_SCORE: f32 = 0.20;
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
    qualified_owner_activity_advanced: bool,
) -> TargetSpeakerUpdate {
    let owner_handoff_suspected = state.local_owner_handoff_suspected;
    // The retained classifier result describes only its own covered window.
    // Once raw capture has moved beyond that window, expose the coverage but
    // do not present the old Target label/quality as if it classified the new
    // audio. This is the callback-order boundary that keeps raw VAD from
    // inheriting a stale positive owner decision.
    let local_speaker_observation_is_current = state
        .local_speaker_observation_end_ms
        .is_some_and(|observation_end_ms| {
            state
                .local_audio_duration_ms
                .is_none_or(|audio_duration_ms| audio_duration_ms <= observation_end_ms)
                && state
                    .local_speech_end_ms
                    .is_none_or(|speech_end_ms| speech_end_ms <= observation_end_ms)
        });
    TargetSpeakerUpdate {
        speaker_id: state.target_speaker_id.clone(),
        target_speech_end_ms: state.target_speech_end_ms,
        provider_audio_duration_ms: state.last_server_audio_duration_ms,
        audio_duration_ms: latest_audio_duration_ms(state),
        local_speech_end_ms: state.local_speech_end_ms,
        qualified_owner_speech_end_ms: state.qualified_owner_speech_end_ms,
        qualified_owner_activity_advanced,
        local_speaker_classification_kind: local_speaker_observation_is_current
            .then(|| {
                state
                    .local_speaker_classification
                    .as_ref()
                    .map(LocalSpeakerClassificationKind::from)
            })
            .flatten(),
        local_speaker_signal_quality_sufficient: local_speaker_observation_is_current
            .then_some(state.local_speaker_signal_quality_sufficient)
            .flatten(),
        local_speaker_observation_end_ms: state.local_speaker_observation_end_ms,
        local_target_speech_end_ms: state.local_target_speech_end_ms,
        // Keep transcript filtering's immediate advisory boundary private.
        // Endpointing sees only the sustained owner-absence boundary so the
        // installed session-2024 two-window owner dip cannot stop recording.
        // A correlated provider-row/voiceprint handoff is an endpoint candidate,
        // not a destructive transcript verdict. Present its live speech edge to
        // the endpoint as non-owner so room speech cannot renew the owner clock.
        local_non_target_speech_end_ms: if owner_handoff_suspected {
            state.local_speech_end_ms
        } else if local_owner_continuity(state) == LocalOwnerContinuity::Other {
            state.local_sustained_non_target_speech_end_ms
        } else {
            None
        },
        local_speaker_tracking_enabled: state.local_speaker_tracking_enabled,
        stable_attributed_speech_end_ms: state.stable_attributed_speech_end_ms,
        target_activity_advanced: target_activity_advanced && !owner_handoff_suspected,
        pending_unattributed_speech: !state.pending_unattributed_text.is_empty(),
        pending_activity_advanced: pending_activity_advanced && !owner_handoff_suspected,
        speaker_info_present: state.speaker_info_present,
    }
}

fn apply_local_speech_evidence_to_update(
    update: &mut TargetSpeakerUpdate,
    evidence: LocalSpeechEvidence,
    captured_audio_samples: u64,
) {
    let captured_audio_ms = captured_audio_samples.saturating_mul(1_000) / 16_000;
    update.audio_duration_ms = Some(captured_audio_ms);

    // The streaming model consumes complete 512-sample windows. Accept only
    // the genuinely unprocessed tail of the current window; a full window of
    // lag is a worker backlog and must remain conservative. Unknown and
    // inconsistent sample clocks never become silence.
    let unanalysed_samples = captured_audio_samples
        .checked_sub(evidence.analyzed_through_samples);
    let analysis_covers_capture = evidence.state != LocalSpeechActivityState::Unknown
        && unanalysed_samples.is_some_and(|samples| samples < 512);
    update.local_speech_end_ms = if !analysis_covers_capture
        || evidence.state == LocalSpeechActivityState::Unknown
    {
        Some(captured_audio_ms)
    } else {
        match evidence.state {
            LocalSpeechActivityState::Speech => Some(captured_audio_ms),
            // A fast detector is allowed to protect the endpoint while the
            // canonical detector confirms the onset.  It is deliberately
            // represented only as a current local speech edge here; manual
            // endpointing consumes it as a bounded hold and no identity or
            // transcript path sees it as a qualified owner watermark.
            LocalSpeechActivityState::PendingSpeech => Some(captured_audio_ms),
            LocalSpeechActivityState::NonSpeech => evidence.last_detected_speech_end_ms,
            LocalSpeechActivityState::Unknown => Some(captured_audio_ms),
        }
    };
}

/// Growing owner-safe ASR activity may refresh the owner endpoint clock only
/// when the shared continuity reducer admits the same revision for preview.
/// Confirmed Other/sustained owner absence blocks both operations. This keeps
/// mid-sentence cross-phrase Uncertain windows inside the already-established
/// owner turn without reviving a turn after the tracker has switched away.
fn refresh_local_target_from_owner_preview_activity(state: &mut SyncState) -> bool {
    if state.local_owner_handoff_suspected
        || !state.local_target_confirmed
        || !local_speaker_allows_owner_endpoint_refresh(state)
    {
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
    // Preview and endpointing must use the identical owner-continuity verdict.
    // A sequential cloud split does not bypass a confirmed Other decision.
    refresh_local_target_from_owner_preview_activity(state)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LocalOwnerContinuity {
    /// No local identity tracker is active; provider policy remains authoritative.
    Untracked,
    /// The newest local window positively matches the enrolled owner.
    Confirmed,
    /// The debounced owner still owns this turn and no sustained owner-absence
    /// evidence exists, but the newest cross-phrase window is inconclusive.
    Compatible,
    /// Current or sustained local evidence identifies a different speaker.
    Other,
    /// Tracking is active but the owner has not yet been established.
    Unknown,
}

/// One identity verdict shared by preview admission and endpoint refresh.
///
/// Session 417 exposed the old split-brain policy: `stable_target + Uncertain`
/// was allowed onto the visible preview while endpointing called the same frame
/// Quiet and stopped twelve milliseconds later. A frame cannot be owner-safe
/// for display yet foreign for capture lifetime. Keep the retained-turn
/// decision here; explicit NonTarget/sustained owner absence still wins.
fn local_owner_continuity(state: &SyncState) -> LocalOwnerContinuity {
    if !state.local_speaker_tracking_enabled {
        return LocalOwnerContinuity::Untracked;
    }
    // An open wake did not match the enrolled bank. After body speech has
    // established a stable session owner, a low-score Uncertain window from
    // that same bank is still inconclusive, even during an absence run. Do not
    // promote it to confirmed Other while the debounced owner has not changed.
    // A current NonTarget window or a correlated handoff remains actionable.
    let open_wake_uncertain_owner = !state.local_wake_owner_verified
        && state.local_speaker_stable_target
        && state.local_target_confirmed
        && state.local_owner_absence_run_confirmed
        && !state.local_owner_handoff_suspected
        && matches!(
            state.local_speaker_classification.as_ref(),
            Some(crate::speaker_verification::SessionSpeakerClassification::Uncertain { .. })
        );
    // The sustained non-target watermark is historical endpoint evidence, not
    // a permanent current-speaker verdict. A later Uncertain window above the
    // owner-absence band ends the confirmed absence run even if it is below
    // the stronger Target threshold (0c67f852: 0.306..0.34 after two low
    // windows). Requiring a new qualified Target here held the growing cloud
    // transcript for almost ten seconds. Keep the watermark for arbitration,
    // while the active absence run and current NonTarget still close this gate.
    let sustained_other_is_current = state
        .local_sustained_non_target_speech_end_ms
        .is_some_and(|other_end_ms| {
            state
                .qualified_owner_speech_end_ms
                .is_none_or(|owner_end_ms| owner_end_ms <= other_end_ms)
                && state
                    .local_speaker_classification
                    .as_ref()
                    .is_none_or(|classification| {
                        classification.score()
                            <= LOCAL_ENDPOINT_OWNER_ABSENCE_CONTINUATION_MAX_SCORE
                    })
        });
    if (!open_wake_uncertain_owner
        && (state.local_owner_absence_run_confirmed || sustained_other_is_current))
        || matches!(
            state.local_speaker_classification.as_ref(),
            Some(crate::speaker_verification::SessionSpeakerClassification::NonTarget { .. })
        )
    {
        return LocalOwnerContinuity::Other;
    }
    // A phrase-only wake can miss the enrolled bank while later body windows
    // positively match it. Once that match is confirmed, keeping the wake
    // flag as a permanent gate makes every subsequent owner preview display
    // only and leaves the endpoint clock blind to resumed speech.
    if !state.local_speaker_stable_target || !state.local_target_confirmed {
        return LocalOwnerContinuity::Unknown;
    }
    match state.local_speaker_classification.as_ref() {
        Some(crate::speaker_verification::SessionSpeakerClassification::Target { .. }) => {
            LocalOwnerContinuity::Confirmed
        }
        Some(crate::speaker_verification::SessionSpeakerClassification::Uncertain { .. })
        | None => LocalOwnerContinuity::Compatible,
        Some(crate::speaker_verification::SessionSpeakerClassification::NonTarget { .. }) => {
            LocalOwnerContinuity::Other
        }
    }
}

fn local_speaker_allows_optimistic_preview(state: &SyncState) -> bool {
    // A debounced in-session Target also establishes display continuity for
    // manual recording. Requiring the automatic-wake flag here suppressed
    // real owner previews even after repeated positive classifications.
    !state.local_speaker_tracking_enabled
        || (!state.local_owner_handoff_suspected
            && state.local_speaker_stable_target
            && local_owner_continuity(state) != LocalOwnerContinuity::Other)
}

fn display_only_provisional_preview_candidate(
    state: &mut SyncState,
    provider_result: &Value,
    _pending_unattributed_speech: bool,
) -> Option<String> {
    // 2026-09-22 15:2x 用户拍板（干扰实测 56eadf8a/5e767040：云端 6 帧胶囊只
    // 亮 1 帧，用户以为死机二次按停→取消吞掉已仲裁终稿）：预览默认假设有干
    // 扰。本通道 display-only，不影响终稿仲裁/端点时钟/插入——身份不确定不
    // 再压黑胶囊，归属判定继续由终稿侧执行。仅当本地稳定判定"当前说话人非
    // 主人"（防抖后的 NonTarget 连续性且无任何主人侧稳定证据）时 withhold。
    let foreign_dominant = state.local_speaker_tracking_enabled
        && local_owner_continuity(state) == LocalOwnerContinuity::Other;
    if foreign_dominant {
        return None;
    }
    // Live 2026-09-11 02:21: capsule empty ~8 s because this helper required
    // pending diarization AND stable owner. Visual preview is display-only
    // (no endpoint refresh). Publish growing provider text as soon as it is
    // longer than what was already shown.
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

/// Temporarily withhold only a new, unclassified provider row at an acoustic
/// identity change. This is not an exclusion verdict: a stable provider row or
/// positive local owner evidence releases it through the normal owner path.
/// A wake-only row cannot arm this hold, so initial body preview remains fast.
fn provider_has_pending_speaker_change(state: &SyncState, result: &Value) -> bool {
    if !state.local_speaker_tracking_enabled {
        return false;
    }
    let Some(rows) = result.get("utterances").and_then(Value::as_array) else {
        return false;
    };
    let phrase_len = spoken_content_len(state.wake_speaker_phrase.as_deref().unwrap_or_default());
    rows.iter().enumerate().any(|(index, row)| {
        if utterance_is_stable(row) && utterance_speaker_id(row).is_some() {
            return false;
        }
        let (Some(start), Some(end)) = (utterance_start_ms(row), utterance_end_ms(row)) else {
            return false;
        };
        let signal_hint = state.local_preview_foreign_hint_end_ms.is_some_and(|hint| {
            hint.saturating_add(LOCAL_SPEAKER_WINDOW_MS) >= start
                && hint <= end.saturating_add(LOCAL_SPEAKER_WINDOW_MS)
        });
        // Short/quiet windows cannot prove a foreign identity. Repeated low
        // scores across a *new provider row* can still ask preview to wait.
        // Reuse the existing absence band/duration; never promote this weaker
        // observation into final exclusion or change the enrolled thresholds.
        let mut low_windows = 0;
        for sample in state.local_speaker_evidence.iter().rev().filter(|sample| {
            sample.audio_end_ms >= start
                && sample.audio_end_ms <= end.saturating_add(LOCAL_SPEAKER_WINDOW_MS)
        }) {
            if sample.classification.score() > LOCAL_OWNER_ABSENCE_MAX_SCORE {
                break;
            }
            low_windows += 1;
        }
        if !signal_hint && low_windows < LOCAL_OWNER_ABSENCE_CONFIRMATIONS {
            return false;
        }
        let prior_body = rows[..index].iter().any(|previous| {
            utterance_is_stable(previous)
                && utterance_end_ms(previous).is_some_and(|end| end <= start)
                && previous
                    .get("text")
                    .and_then(Value::as_str)
                    .is_some_and(|text| spoken_content_len(text) > phrase_len)
        });
        let owner_returned = state.local_speaker_evidence.iter().any(|sample| {
            let center = sample
                .audio_end_ms
                .saturating_sub(LOCAL_SPEAKER_WINDOW_MS / 2);
            center >= start
                && center <= end
                && sample.stable_target
                && matches!(
                    sample.classification,
                    crate::speaker_verification::SessionSpeakerClassification::Target { .. }
                )
        });
        prior_body && !owner_returned
    })
}

fn provider_has_locally_excluded_later_utterance(state: &SyncState, result: &Value) -> bool {
    provider_has_locally_excluded_later_utterance_with_stability(state, result, false)
}

fn provider_has_locally_excluded_later_utterance_with_stability(
    state: &SyncState,
    result: &Value,
    require_stable_foreign_row: bool,
) -> bool {
    if !state.local_speaker_tracking_enabled {
        return false;
    }
    let Some(utterances) = result.get("utterances").and_then(Value::as_array) else {
        return false;
    };
    let phrase = state.wake_speaker_phrase.as_deref();
    let normalized_phrase = phrase
        .unwrap_or_default()
        .chars()
        .filter(|ch| ch.is_alphanumeric())
        .collect::<String>();
    utterances.iter().enumerate().any(|(index, utterance)| {
        if require_stable_foreign_row && !utterance_is_stable(utterance) {
            return false;
        }
        let Some(start_ms) = utterance_start_ms(utterance) else {
            return false;
        };
        // A different cloud speaker, or a provisional row with no speaker id,
        // can corroborate a sustained local mismatch. The same stable cloud
        // speaker cannot: post-pause owner speech produces the same low scores.
        let provider_indicates_other = match utterance_speaker_id(utterance) {
            Some(speaker) => state
                .target_speaker_id
                .as_deref()
                .is_some_and(|target| speaker != target),
            None => !utterance_is_stable(utterance),
        };
        if !provider_indicates_other {
            return false;
        }
        // Physical 7ba9b684: the new speaker's streaming row had no speaker id
        // for seven seconds. A provisional suspicion can protect preview
        // arbitration; final destructive exclusion still needs cloud support.
        let established_body_before = utterances[..index].iter().any(|previous| {
            let (Some(previous_start), Some(previous_end)) =
                (utterance_start_ms(previous), utterance_end_ms(previous))
            else {
                return false;
            };
            if !utterance_is_stable(previous) || previous_end > start_ms {
                return false;
            }
            // A provider row can contain both wake and a long owner body.
            // Excluding that whole row from calibration made late foreign
            // rows eligible for the same-speaker recovery path forever.
            // Ignore the wake window itself: it is not body calibration.
            let body_start = if utterance_contains_normalized_phrase(previous, &normalized_phrase) {
                previous_start.saturating_add(MAX_WAKE_PHRASE_UTTERANCE_MS)
            } else {
                previous_start
            };
            body_start < previous_end
                && local_evidence_allows_utterance(
                    &json!({"start_time": body_start, "end_time": previous_end}),
                    &state.local_speaker_evidence,
                    None,
                )
        });
        if !established_body_before {
            return false;
        }
        let Some(end_ms) = utterance_end_ms(utterance) else {
            return false;
        };
        // A short stable foreign row may seal before the 1.2 s verifier
        // window arrives. Give it only one window of look-ahead.
        let evidence_end_ms = if utterance_is_stable(utterance)
            && end_ms.saturating_sub(start_ms) < LOCAL_SPEAKER_WINDOW_MS
        {
            end_ms.saturating_add(LOCAL_SPEAKER_WINDOW_MS)
        } else {
            end_ms
        };
        let mut low_votes = 0u32;
        for sample in &state.local_speaker_evidence {
            let center_ms = sample.audio_end_ms.saturating_sub(LOCAL_SPEAKER_WINDOW_MS / 2);
            if center_ms < start_ms || center_ms > evidence_end_ms {
                continue;
            }
            if matches!(
                sample.classification,
                crate::speaker_verification::SessionSpeakerClassification::Target { .. }
            ) {
                return false;
            }
            if !sample.stable_target || sample.classification.score() <= LOCAL_OWNER_ABSENCE_MAX_SCORE {
                low_votes += 1;
            }
        }
        low_votes >= LOCAL_OWNER_ABSENCE_CONFIRMATIONS
    })
}

fn update_local_preview_exclusion(
    state: &mut SyncState,
    result: &Value,
) -> bool {
    // A provisional speakerless row is a frame-level hint; latching it for
    // the whole session made the owner's next clause unable to refresh the
    // endpoint and wrongly forced the final arbiter into foreign-tail mode.
    state.local_preview_exclusion_seen |=
        provider_has_locally_excluded_later_utterance_with_stability(state, result, true);
    provider_has_locally_excluded_later_utterance(state, result)
        || state.local_preview_exclusion_seen
}

fn local_speaker_allows_owner_endpoint_refresh(state: &SyncState) -> bool {
    !state.local_owner_handoff_suspected
        && matches!(
            local_owner_continuity(state),
            LocalOwnerContinuity::Confirmed | LocalOwnerContinuity::Compatible
        )
}

#[derive(Clone)]
struct AudioSourceRun {
    bytes: usize,
    observation: Option<Arc<crate::observability::EmbeddedAudioPipelineObservation>>,
    segment_id: Option<u32>,
    destination_range: Option<crate::observability::PcmRange>,
    source_interval: Option<crate::observability::PcmSourceInterval>,
}

#[derive(Clone)]
struct AudioSourceShare {
    observation: Option<Arc<crate::observability::EmbeddedAudioPipelineObservation>>,
    segment_id: Option<u32>,
    bytes: usize,
    destination_range: Option<crate::observability::PcmRange>,
    source_interval: Option<crate::observability::PcmSourceInterval>,
}

fn append_audio_source_run(
    runs: &mut VecDeque<AudioSourceRun>,
    bytes: usize,
    observation: Option<Arc<crate::observability::EmbeddedAudioPipelineObservation>>,
    segment_id: Option<u32>,
    destination_range: Option<crate::observability::PcmRange>,
    source_interval: Option<crate::observability::PcmSourceInterval>,
) {
    if bytes == 0 {
        return;
    }
    let can_merge = runs.back().is_some_and(|last| {
        last.segment_id == segment_id
            && match (last.source_interval, source_interval) {
                (Some(left), Some(right)) => left.same_stream_and_adjacent(right),
                (None, None) => true,
                _ => false,
            }
            && match (last.destination_range, destination_range) {
                (Some(left), Some(right)) => left.is_adjacent_to(right),
                (None, None) => true,
                _ => false,
            }
            && match (&last.observation, &observation) {
                (Some(left), Some(right)) => Arc::ptr_eq(left, right),
                (None, None) => true,
                _ => false,
            }
    });
    if can_merge {
        let last = runs.back_mut().expect("source run exists");
        last.bytes = last.bytes.saturating_add(bytes);
        if let (Some(last_interval), Some(interval)) =
            (last.source_interval.as_mut(), source_interval)
        {
            last_interval.range.end = interval.range.end;
        }
        if let (Some(last_range), Some(range)) =
            (last.destination_range.as_mut(), destination_range)
        {
            last_range.end = range.end;
        }
        return;
    }
    runs.push_back(AudioSourceRun {
        bytes,
        observation,
        segment_id,
        destination_range,
        source_interval,
    });
}

fn drain_audio_source_runs(
    runs: &mut VecDeque<AudioSourceRun>,
    bytes: usize,
) -> Vec<AudioSourceShare> {
    let mut remaining = bytes;
    let mut shares: Vec<AudioSourceShare> = Vec::new();
    while remaining > 0 {
        let Some(mut run) = runs.pop_front() else {
            break;
        };
        let portion = run.bytes.min(remaining);
        if portion > 0 {
            let portion_interval = if let Some(interval) = run.source_interval.as_mut() {
                match interval.take_prefix(portion) {
                    Some(interval) => Some(interval),
                    None => {
                        run.observation
                            .as_ref()
                            .map(|observation| observation.mark_source_interval_incomplete());
                        run.source_interval = None;
                        None
                    }
                }
            } else {
                None
            };
            let portion_destination_range = if let Some(range) = run.destination_range.as_mut() {
                match range.take_prefix(portion) {
                    Some(range) => Some(range),
                    None => {
                        run.observation
                            .as_ref()
                            .map(|observation| observation.mark_asr_destination_facts_incomplete());
                        run.destination_range = None;
                        None
                    }
                }
            } else {
                None
            };
            let can_merge = shares.last().is_some_and(|last| {
                last.segment_id == run.segment_id
                    && match (last.source_interval, portion_interval) {
                        (Some(left), Some(right)) => left.same_stream_and_adjacent(right),
                        (None, None) => true,
                        _ => false,
                    }
                    && match (last.destination_range, portion_destination_range) {
                        (Some(left), Some(right)) => left.is_adjacent_to(right),
                        (None, None) => true,
                        _ => false,
                    }
                    && match (&last.observation, &run.observation) {
                        (Some(left), Some(right)) => Arc::ptr_eq(left, right),
                        (None, None) => true,
                        _ => false,
                    }
            });
            if can_merge {
                let last = shares.last_mut().expect("source share exists");
                last.bytes = last.bytes.saturating_add(portion);
                if let (Some(last_interval), Some(interval)) =
                    (last.source_interval.as_mut(), portion_interval)
                {
                    last_interval.range.end = interval.range.end;
                }
                if let (Some(last_range), Some(range)) =
                    (last.destination_range.as_mut(), portion_destination_range)
                {
                    last_range.end = range.end;
                }
            } else {
                shares.push(AudioSourceShare {
                    observation: run.observation.clone(),
                    segment_id: run.segment_id,
                    bytes: portion,
                    destination_range: portion_destination_range,
                    source_interval: portion_interval,
                });
            }
            remaining -= portion;
            run.bytes -= portion;
        }
        if run.bytes > 0 {
            runs.push_front(run);
        }
    }
    shares
}

fn for_each_source_group<F>(shares: &[AudioSourceShare], mut visit: F)
where
    F: FnMut(
        &Arc<crate::observability::EmbeddedAudioPipelineObservation>,
        &[(Option<u32>, usize)],
        usize,
    ),
{
    let mut groups: Vec<(
        Arc<crate::observability::EmbeddedAudioPipelineObservation>,
        Vec<(Option<u32>, usize)>,
        usize,
    )> = Vec::new();
    for share in shares {
        let Some(observation) = share.observation.as_ref() else {
            continue;
        };
        let Some((_, group_shares, group_bytes)) = groups
            .iter_mut()
            .find(|(existing, _, _)| Arc::ptr_eq(existing, observation))
        else {
            groups.push((
                Arc::clone(observation),
                vec![(share.segment_id, share.bytes)],
                share.bytes,
            ));
            continue;
        };
        let can_merge = group_shares
            .last()
            .is_some_and(|(last_segment_id, _)| *last_segment_id == share.segment_id);
        if can_merge {
            let (_, last_bytes) = group_shares.last_mut().expect("source share exists");
            *last_bytes = (*last_bytes).saturating_add(share.bytes);
        } else {
            group_shares.push((share.segment_id, share.bytes));
        }
        *group_bytes = group_bytes.saturating_add(share.bytes);
    }
    for (observation, group_shares, group_bytes) in groups {
        visit(&observation, &group_shares, group_bytes);
    }
}

fn for_each_observation_share_group<F>(shares: &[AudioSourceShare], mut visit: F)
where
    F: FnMut(&Arc<crate::observability::EmbeddedAudioPipelineObservation>, &[AudioSourceShare]),
{
    let mut groups: Vec<(
        Arc<crate::observability::EmbeddedAudioPipelineObservation>,
        Vec<AudioSourceShare>,
    )> = Vec::new();
    for share in shares {
        let Some(observation) = share.observation.as_ref() else {
            continue;
        };
        let Some((_, group_shares)) = groups
            .iter_mut()
            .find(|(existing, _)| Arc::ptr_eq(existing, observation))
        else {
            groups.push((Arc::clone(observation), vec![share.clone()]));
            continue;
        };
        group_shares.push(share.clone());
    }
    for (observation, group_shares) in groups {
        visit(&observation, &group_shares);
    }
}

fn destination_range_for_source_shares(
    shares: &[AudioSourceShare],
    frame_bytes: usize,
) -> Option<crate::observability::PcmRange> {
    let first = shares.first()?.destination_range?;
    let mut expected_start = first.start;
    let mut total_bytes = 0_u64;
    for share in shares {
        let range = share.destination_range?;
        let share_bytes = u64::try_from(share.bytes).ok()?;
        if range.start != expected_start || range.end.checked_sub(range.start) != Some(share_bytes)
        {
            return None;
        }
        expected_start = range.end;
        total_bytes = total_bytes.checked_add(share_bytes)?;
    }
    let frame_bytes = u64::try_from(frame_bytes).ok()?;
    (total_bytes == frame_bytes && expected_start.checked_sub(first.start) == Some(frame_bytes))
        .then_some(crate::observability::PcmRange {
            start: first.start,
            end: expected_start,
        })
}

fn asr_source_share_facts(
    shares: &[AudioSourceShare],
) -> Vec<crate::observability::AsrSourceShareFact> {
    shares
        .iter()
        .map(|share| crate::observability::AsrSourceShareFact {
            segment_id: share.segment_id,
            bytes: share.bytes as u64,
            destination_range: share.destination_range,
            source_interval: share.source_interval,
            collector_metadata: None,
            collector_emitted_range: None,
        })
        .collect()
}

fn record_asr_destination_fact_for_source_shares(
    asr_stream_id: u64,
    sequence: Option<i32>,
    destination_range: Option<crate::observability::PcmRange>,
    outcome: crate::observability::AsrDestinationOutcome,
    shares: &[AudioSourceShare],
) {
    for_each_observation_share_group(shares, |observation, group_shares| {
        observation.record_asr_destination_fact(crate::observability::AsrDestinationFact {
            asr_stream_id,
            sequence,
            destination_range,
            outcome,
            source_shares: asr_source_share_facts(group_shares),
        });
    });
}

fn record_asr_queued_for_source_shares(
    asr_stream_id: u64,
    sequence: i32,
    destination_range: Option<crate::observability::PcmRange>,
    shares: &[AudioSourceShare],
    frame_bytes: usize,
) {
    record_asr_destination_fact_for_source_shares(
        asr_stream_id,
        Some(sequence),
        destination_range,
        crate::observability::AsrDestinationOutcome::Queued,
        shares,
    );
    for_each_source_group(shares, |observation, segment_shares, group_bytes| {
        observation.record_asr_queued_for_sources(segment_shares, group_bytes.min(frame_bytes));
    });
}

fn record_asr_claimed_for_source_shares(
    asr_stream_id: u64,
    sequence: i32,
    destination_range: Option<crate::observability::PcmRange>,
    shares: &[AudioSourceShare],
) {
    record_asr_destination_fact_for_source_shares(
        asr_stream_id,
        Some(sequence),
        destination_range,
        crate::observability::AsrDestinationOutcome::Claimed,
        shares,
    );
}

fn record_asr_queue_rejected_for_source_shares(
    asr_stream_id: u64,
    sequence: i32,
    destination_range: Option<crate::observability::PcmRange>,
    shares: &[AudioSourceShare],
    frame_bytes: usize,
) {
    record_asr_destination_fact_for_source_shares(
        asr_stream_id,
        Some(sequence),
        destination_range,
        crate::observability::AsrDestinationOutcome::QueueRejected,
        shares,
    );
    for_each_source_group(shares, |observation, segment_shares, group_bytes| {
        observation
            .record_asr_queue_rejected_for_sources(segment_shares, group_bytes.min(frame_bytes));
    });
}

fn record_asr_queue_rejected_without_queue_for_source_shares(
    asr_stream_id: u64,
    sequence: i32,
    destination_range: Option<crate::observability::PcmRange>,
    shares: &[AudioSourceShare],
    frame_bytes: usize,
) {
    record_asr_destination_fact_for_source_shares(
        asr_stream_id,
        Some(sequence),
        destination_range,
        crate::observability::AsrDestinationOutcome::QueueRejectedWithoutQueue,
        shares,
    );
    for_each_source_group(shares, |observation, segment_shares, group_bytes| {
        observation.record_asr_queue_rejected_without_queue_for_sources(
            segment_shares,
            group_bytes.min(frame_bytes),
        );
    });
}

fn record_asr_send_completed_for_source_shares(
    asr_stream_id: u64,
    sequence: i32,
    destination_range: Option<crate::observability::PcmRange>,
    shares: &[AudioSourceShare],
    frame_bytes: usize,
) {
    record_asr_destination_fact_for_source_shares(
        asr_stream_id,
        Some(sequence),
        destination_range,
        crate::observability::AsrDestinationOutcome::SocketSendCompleted,
        shares,
    );
    for_each_source_group(shares, |observation, segment_shares, group_bytes| {
        observation
            .record_asr_send_completed_for_sources(segment_shares, group_bytes.min(frame_bytes));
    });
}

fn record_asr_send_failed_for_source_shares(
    asr_stream_id: u64,
    sequence: i32,
    destination_range: Option<crate::observability::PcmRange>,
    shares: &[AudioSourceShare],
    frame_bytes: usize,
) {
    record_asr_destination_fact_for_source_shares(
        asr_stream_id,
        Some(sequence),
        destination_range,
        crate::observability::AsrDestinationOutcome::SendFailed,
        shares,
    );
    for_each_source_group(shares, |observation, segment_shares, group_bytes| {
        observation
            .record_asr_send_failed_for_sources(segment_shares, group_bytes.min(frame_bytes));
    });
}

fn record_asr_abandoned_for_source_shares(
    asr_stream_id: u64,
    sequence: i32,
    destination_range: Option<crate::observability::PcmRange>,
    shares: &[AudioSourceShare],
    frame_bytes: usize,
) {
    record_asr_destination_fact_for_source_shares(
        asr_stream_id,
        Some(sequence),
        destination_range,
        crate::observability::AsrDestinationOutcome::Abandoned,
        shares,
    );
    for_each_source_group(shares, |observation, segment_shares, group_bytes| {
        observation.record_asr_abandoned_for_sources(segment_shares, group_bytes.min(frame_bytes));
    });
}

fn record_asr_unresolved_for_source_shares(
    asr_stream_id: u64,
    destination_range: Option<crate::observability::PcmRange>,
    shares: &[AudioSourceShare],
    bytes: usize,
) {
    record_asr_destination_fact_for_source_shares(
        asr_stream_id,
        None,
        destination_range,
        crate::observability::AsrDestinationOutcome::BufferedUnresolved,
        shares,
    );
    for_each_source_group(shares, |observation, segment_shares, group_bytes| {
        observation.record_asr_unresolved_for_sources(segment_shares, group_bytes.min(bytes));
    });
}

/// Convert a momentary provider/local disagreement into a session-level endpoint
/// candidate. The provider may later assign the new row the owner's old speaker
/// id, so recomputing this from each response would lose the only trustworthy
/// boundary and let the foreign tail resume the owner clock.
fn observe_provider_owner_handoff_candidate(state: &mut SyncState, result: &Value) -> bool {
    if state.local_owner_handoff_suspected {
        return true;
    }
    if !state.local_wake_owner_verified
        || state.local_speaker_profile_adaptive
        || !state.local_target_confirmed
        || !provider_has_pending_speaker_change(state, result)
    {
        return false;
    }
    state.local_owner_handoff_suspected = true;
    state.local_owner_handoff_recovery_targets = 0;
    log::info!(
        "[asr] owner handoff candidate froze endpoint renewal local_speech_end_ms={:?} owner_end_ms={:?}",
        state.local_speech_end_ms,
        state.local_target_speech_end_ms
    );
    true
}

/// A correlated handoff candidate is deliberately reversible. Require two new
/// consecutive Target windows so one mixed/overlapping frame cannot hand the
/// endpoint back to a room speaker that happens to resemble the owner.
fn observe_owner_handoff_recovery(
    state: &mut SyncState,
    classification: crate::speaker_verification::SessionSpeakerClassification,
) -> bool {
    if !state.local_owner_handoff_suspected {
        return false;
    }
    if matches!(
        classification,
        crate::speaker_verification::SessionSpeakerClassification::Target { .. }
    ) {
        state.local_owner_handoff_recovery_targets =
            state.local_owner_handoff_recovery_targets.saturating_add(1);
    } else {
        state.local_owner_handoff_recovery_targets = 0;
    }
    if state.local_owner_handoff_recovery_targets < LOCAL_SPEAKER_SWITCH_CONFIRMATIONS {
        return false;
    }
    state.local_owner_handoff_suspected = false;
    state.local_owner_handoff_recovery_targets = 0;
    log::info!("[asr] consecutive owner evidence released endpoint handoff candidate");
    true
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

// POST-STOP 尾巴豁免的最小新音频量（2026-09-23 5ac70fc7）：真实续说会让
// 终稿覆盖超出 STOP 水位数秒（该会话 +4.5s）；而两遍精修/幻听不产生新
// 音频，drain 兜底最多带出 ~1s 排队帧。1.5s 把两者分开。
const POST_STOP_OWNER_TAIL_MIN_NEW_AUDIO_MS: u64 = 1_500;

/// Once the host has sent the stop boundary, the capsule's last owner-safe
/// preview is the only text the user has actually seen.  Volcengine's
/// authoritative two-pass frame can still arrive later and append an
/// un-attributed tail (the observed 45 -> 55 character regression).  Accepting
/// that late growth makes the final insertion differ from the preview and is
/// exactly how room speech/recognition hallucinations leak into the user's
/// text.  Keep punctuation-only corrections, but never admit new spoken
/// content after the boundary unless a separately extracted owner track exists.
///
/// 2026-09-23 续接豁免（5ac70fc7 实锤）：用户在 3s 窗口内续说、端点竞速
/// 输了照常 STOP——尾音经 drain 上云，owner 过滤终稿完整含尾巴（58/45
/// 字）。这种"晚到的增长"有新捕获音频背书（终稿覆盖显著超过 STOP 水
/// 位），与无新音频的两遍幻听本质不同：保留尾巴，交付层（pause-early
/// 余量/锚点）负责只贴增量。
fn owner_preview_safety_ceiling(
    state: &SyncState,
    merged: &str,
    explicit_non_owner_tail: bool,
) -> Option<String> {
    if !state.finishing
        || !state.local_speaker_tracking_enabled
        || state.last_emitted_preview_text.trim().is_empty()
        || spoken_content_len(merged) <= spoken_content_len(&state.last_emitted_preview_text)
        || state.wake_bound_single_speaker_final_text.is_some()
        || state.distinct_speaker_target_final_text.is_some()
    {
        return None;
    }
    // 豁免：终稿云端覆盖比 STOP 水位多出真实新音频 → 增长是抓到的语音
    // 而不是幻听，不砍。无水位（旧会话路径/直发终稿）不豁免，维持保守。
    if let Some(boundary_ms) = state.stop_boundary_server_audio_ms {
        let covered_ms = state
            .last_server_audio_duration_ms
            .into_iter()
            .chain(state.local_audio_duration_ms)
            .max();
        if covered_ms.is_some_and(|now| {
            now.saturating_sub(boundary_ms) >= POST_STOP_OWNER_TAIL_MIN_NEW_AUDIO_MS
        }) {
            log::info!(
                "[asr] post-stop owner tail backed by {}ms of newly covered audio; keeping late tail preview_chars={} final_chars={}",
                covered_ms
                    .map(|now| now.saturating_sub(boundary_ms))
                    .unwrap_or(0),
                state.last_emitted_preview_text.chars().count(),
                merged.chars().count()
            );
            return None;
        }
    }
    let normalize_spoken = |text: &str| {
        text.chars()
            .filter(|ch| ch.is_alphanumeric())
            .collect::<String>()
    };
    let preview = state.last_emitted_preview_text.trim();
    // The first provisional token can be a foreign fragment before the real
    // wake phrase (installed 1.0.6 session 6c62818a: "Her." before "开始录音").
    // It is not an owner boundary and must not consume the corrected final's
    // character allowance.
    let aligned_preview = state
        .wake_speaker_phrase
        .as_deref()
        .and_then(|phrase| {
            let offset = preview.find(phrase)?;
            merged.starts_with(phrase).then_some(&preview[offset..])
        })
        .unwrap_or(preview);
    let normalized_preview = normalize_spoken(aligned_preview);
    let normalized_merged = normalize_spoken(merged);
    if normalized_preview.is_empty() || normalized_merged == normalized_preview {
        // Same spoken content with different punctuation is a legitimate final
        // correction and should not be thrown away.
        return None;
    }
    if !normalized_merged.starts_with(&normalized_preview) && !explicit_non_owner_tail {
        // A two-pass correction changed the earlier words. Length alone is
        // not proof of a late foreign append; replacing the authoritative
        // final with the stale preview loses corrections and punctuation.
        return None;
    }
    let content_limit = normalized_preview.chars().count();
    let mut content_seen = 0usize;
    let mut boundary = merged.len();
    for (offset, ch) in merged.char_indices() {
        if ch.is_alphanumeric() {
            if content_seen == content_limit {
                boundary = offset;
                break;
            }
            content_seen += 1;
        }
    }
    if content_seen < content_limit || boundary == merged.len() {
        return None;
    }
    let corrected_owner_prefix = merged[..boundary].trim_end().to_string();
    log::warn!(
        "[asr] post-stop final exceeded owner preview; capping corrected final to owner content preview_chars={} final_chars={} kept_chars={} explicit_non_owner={}",
        preview.chars().count(),
        merged.chars().count(),
        corrected_owner_prefix.chars().count(),
        explicit_non_owner_tail,
    );
    Some(corrected_owner_prefix)
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
/// have fresh local owner evidence, and every overlapping local sample must
/// remain above the owner-absence boundary without a single NonTarget
/// classification. A latched `stable_target` bit by itself is not evidence for
/// a later time interval. This also covers provider A/B/A cluster drift.
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
        || state.owner_isolation_frozen
        || target_text.trim().is_empty()
    {
        return false;
    }
    // The identity-latch conditions below used to be a hard gate. Under
    // sustained background media (e57053aa family, 2026-09-19) the latch is
    // exactly what the interference erodes — the owner's resumed sentence
    // carries only media-mixed Uncertain windows and the pause fills with
    // confirmed NonTarget media — so gate on them only when no verified wake
    // owner can supply the contrastive authority. The per-window checks below
    // still demand their own evidence; nothing here accepts a split row whose
    // windows are not positively owned or contrastively owner-like.
    if !(state.local_speaker_stable_target && state.local_consecutive_non_target == 0)
        && !state.local_wake_owner_verified
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
    let normalized_attributed_target = normalize(&attributed_target_text);
    let original_target_only_shape = normalized_attributed_target == normalized_target;
    let response_local_body_alias_shape = normalized_target
        .starts_with(&normalized_attributed_target)
        && normalized_attributed.starts_with(&normalized_target);
    // The response-local immediate-body alias may already have admitted the
    // first safe split cluster. In that shape `target_text` is a contiguous
    // prefix longer than the provider's original wake-cluster text. Preserve
    // the existing all-split local-evidence validation below so later clusters
    // (and a final unsegmented suffix) are recovered only when every window is
    // still owned and no NonTarget veto exists.
    if !original_target_only_shape && !response_local_body_alias_shape {
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
        if overlapping.is_empty()
            || local_non_target_vetoes_split_utterance(state, start_ms, end_ms)
        {
            return false;
        }
        if overlapping.iter().all(|sample| {
            sample.stable_target
                && sample.classification.score() > LOCAL_OWNER_ABSENCE_MAX_SCORE
                && !matches!(
                    sample.classification,
                    crate::speaker_verification::SessionSpeakerClassification::NonTarget { .. }
                )
        }) {
            return true;
        }
        // Media-time contrastive alternative: the split row's windows are
        // dragged into Uncertain by the interference the same session also
        // confirms as NonTarget in its gaps. Accept only on the relative
        // contrast (no non-owner window inside the row, best window clears the
        // session's interference ceiling) and only for rows that start after
        // the wake-attributed target audio — a bystander speaking before or
        // over the owner keeps the strict contract above.
        state.local_wake_owner_verified
            && target_utterances
                .iter()
                .filter_map(|target| utterance_start_ms(target))
                .min()
                .is_some_and(|first_target_start_ms| start_ms >= first_target_start_ms)
            && local_evidence_contrastive_owner_continuation(
                &state.local_speaker_evidence,
                start_ms,
                end_ms,
            )
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
                && sample.classification.score() > LOCAL_OWNER_ABSENCE_MAX_SCORE
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

    // Transcript-grade mismatch evidence is intentionally advisory: it must
    // not erase text by itself. Once two consecutive windows agree, however,
    // a provider-final fail-open path may not use the overlapping foreign
    // speaker row to grow the owner result. Session 2744 had exactly this
    // shape: provider speaker 0 ended at 8192 ms, speaker 1 occupied
    // 8272..9702 ms, and the confirmed hard mismatch was centred at 8800 ms.
    if state
        .local_confirmed_transcript_non_target_end_ms
        .is_some_and(|audio_end_ms| inside(sample_center(audio_end_ms)))
    {
        return true;
    }

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

fn stable_provider_foreign_row_has_local_veto(state: &SyncState, provider_result: &Value) -> bool {
    let Some(target_speaker_id) = state.target_speaker_id.as_deref() else {
        return false;
    };
    let sample_center =
        |audio_end_ms: u64| audio_end_ms.saturating_sub(LOCAL_SPEAKER_WINDOW_MS / 2);
    provider_result
        .get("utterances")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|utterance| utterance_is_stable(utterance))
        .filter(|utterance| {
            utterance_speaker_id(utterance)
                .as_deref()
                .is_some_and(|speaker_id| speaker_id != target_speaker_id)
        })
        .any(|utterance| {
            utterance_start_ms(utterance)
                .zip(utterance_end_ms(utterance))
                .is_some_and(|(start_ms, end_ms)| {
                    let confirmed_short_foreign_hint_overlaps = state
                        .local_confirmed_transcript_foreign_hint_end_ms
                        .is_some_and(|audio_end_ms| {
                            let center_ms = sample_center(audio_end_ms);
                            center_ms >= start_ms && center_ms <= end_ms
                        });
                    confirmed_short_foreign_hint_overlaps
                        || local_non_target_vetoes_split_utterance(state, start_ms, end_ms)
                })
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
/// Only return text that has entered the authoritative session ledger. The
/// optimistic preview is display-only: it may be useful while attribution is
/// pending, but an empty/weak protocol final must never promote that text into
/// final recovery merely because it is longer than the committed ledger.
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
    if state.best_transcript_text != text {
        state.best_transcript_committed_at = Some(Instant::now());
    }
    state.best_transcript_text = text.clone();
    state.best_transcript_segments = segments;
    state.last_partial_text = text;
}

fn rolling_stable_transcript_prefix(
    revisions: &mut VecDeque<(Instant, String)>,
    current: &str,
    changed_at: Instant,
    now: Instant,
    min_stable: Duration,
) -> Option<String> {
    if current.trim().is_empty() {
        revisions.clear();
        return None;
    }
    if revisions
        .back()
        .is_none_or(|(at, text)| *at != changed_at || text != current)
    {
        revisions.push_back((changed_at, current.to_string()));
    }
    let cutoff = now.checked_sub(min_stable)?;
    // Keep the newest revision before the cutoff plus every later revision.
    // All of them must agree on the prefix before it can leave the app.
    while revisions.len() > 1 && revisions.get(1).is_some_and(|(at, _)| *at <= cutoff) {
        revisions.pop_front();
    }
    while revisions.len() > 64 {
        revisions.pop_front();
    }
    if revisions.front().is_none_or(|(at, _)| *at > cutoff) {
        return None;
    }
    let mut stable = revisions.front()?.1.clone();
    for (_, revision) in revisions.iter().skip(1) {
        stable = stable
            .chars()
            .zip(revision.chars())
            .take_while(|(left, right)| left == right)
            .map(|(ch, _)| ch)
            .collect();
        if stable.is_empty() {
            return None;
        }
    }
    Some(stable)
}

fn provider_raw_fallback_allowed(state: &SyncState) -> bool {
    if !state.local_speaker_tracking_enabled {
        return true;
    }
    // Local voiceprint negatives are endpoint/advisory evidence. They do not
    // identify a second speaker by themselves: session 2024 had two such
    // windows while Volcengine had no diarization rows, and its provider text
    // remained the only accepted body. A provider/local correlated handoff or
    // an already-frozen owner ceiling is the explicit veto.
    !state.owner_isolation_frozen && !state.local_owner_handoff_suspected
}

/// A target-speaker extraction pass can legitimately return no selected row
/// when the provider's stable utterance starts after the wake row or when the
/// extraction chunk is masked by interference.  If there is no explicit
/// non-owner/absence evidence, replacing a non-empty provider final with that
/// empty filtered result silently drops the owner's tail (observed 36 cloud
/// chars collapsing to a 26-char session ledger).  Keep the provider final in
/// this narrow fail-open shape; hard isolation and any explicit other-speaker
/// evidence still win elsewhere.
fn final_unfiltered_provider_recovery_allowed(
    state: &SyncState,
    filtered: &SpeakerFilteredResult,
    provider_result: &Value,
) -> bool {
    let provider_text = provider_result
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if provider_text.trim().is_empty() {
        return false;
    }
    let filtered_text = filtered
        .result
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default();
    // With no local voiceprint, a cloud diarization row is only partial
    // attribution metadata, not an owner isolation decision. If the final
    // provider text is longer and there is no explicit other-speaker evidence,
    // preserve the complete provider transcript instead of committing only
    // the first attributed row (the observed 84 -> 4 char collapse).
    if !state.local_speaker_tracking_enabled {
        return spoken_content_len(provider_text) > spoken_content_len(filtered_text);
    }
    if filtered.stable_non_target_utterance_present
        || filtered.stable_other_speaker_present
        || filtered.stable_unresolved_speaker_present
        || state.owner_isolation_frozen
        || !provider_raw_fallback_allowed(state)
    {
        return false;
    }
    filtered_text.trim().is_empty()
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

/// Assemble the wake-anchored session-ownership evidence for the pure kernel
/// decision `open_session_wake_owned_body_can_recover`. The facts are purely
/// textual/session-state: the filter kept exactly the wake phrase while the
/// provider transcript starts with that phrase and carries a substantial body.
fn open_session_wake_owned_body_recovery(
    state: &SyncState,
    filtered: &SpeakerFilteredResult,
    provider_result: &Value,
) -> bool {
    let normalize = |text: &str| {
        text.chars()
            .filter(|ch| ch.is_alphanumeric())
            .collect::<String>()
    };
    let target_text = filtered
        .result
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let provider_text = provider_result
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let normalized_target = normalize(target_text);
    let normalized_provider = normalize(provider_text);
    let normalized_phrase = state
        .wake_speaker_phrase
        .as_deref()
        .map(|phrase| normalize(phrase))
        .unwrap_or_default();
    crate::speech_decision_kernel::open_session_wake_owned_body_can_recover(
        crate::speech_decision_kernel::OpenSessionWakeOwnedBodyEvidence {
            tracking_enabled: state.local_speaker_tracking_enabled,
            wake_owner_verified: state.local_wake_owner_verified,
            owner_isolation_frozen: state.owner_isolation_frozen,
            hard_non_target_latched: state.local_non_target_speech_end_ms.is_some(),
            filtered_is_wake_only: !normalized_target.is_empty()
                && !normalized_phrase.is_empty()
                && normalized_target == normalized_phrase,
            provider_has_wake_anchored_body: !normalized_phrase.is_empty()
                && normalized_provider.starts_with(&normalized_phrase)
                && normalized_provider.len()
                    >= normalized_phrase.len()
                        + crate::speech_decision_kernel::OPEN_SESSION_WAKE_OWNED_MIN_BODY_CHARS,
        },
    )
}

/// A stable provider row that is not the verified target (or its tightly
/// contiguous body alias) is already explicit foreign-speaker evidence.  It
/// must reach the single final arbiter even when the local verifier has not
/// yet debounced a NonTarget window; otherwise an owner-recovery branch can
/// re-introduce the foreign row into the committed transcript.
fn final_explicit_non_owner_tail(
    state: &SyncState,
    filtered: &SpeakerFilteredResult,
    provider_result: &Value,
) -> bool {
    // 2026-09-20 0dbc59da/86768c0e: a wake that itself failed bank
    // verification cannot license the same verifier to veto the
    // wake-anchored body as foreign. See the kernel decision for the full
    // rationale; a latched hard NonTarget window still vetoes inside it.
    if open_session_wake_owned_body_recovery(state, filtered, provider_result) {
        return false;
    }
    let target_text = filtered
        .result
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let cloud_row_is_verified_owner_continuation = filtered.stable_other_speaker_present
        && (sequential_speaker_split_gap_is_owner_safe(state, provider_result, target_text)
            || final_wake_only_provider_gap_is_owner_safe(state, provider_result, target_text)
            || final_unsegmented_provider_tail_is_owner_safe(state, provider_result, target_text));
    state.local_preview_exclusion_seen
        || filtered.stable_non_target_utterance_present
        || (filtered.stable_other_speaker_present && !cloud_row_is_verified_owner_continuation)
        || state.owner_isolation_frozen
        || stable_provider_foreign_row_has_local_veto(state, provider_result)
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
    response_local_body_alias_present: bool,
    stable_non_target_utterance_present: bool,
    stable_other_speaker_present: bool,
    stable_unresolved_speaker_present: bool,
    target_speech_end_ms: Option<u64>,
    wake_target_speech_end_ms: Option<u64>,
    stable_attributed_speech_end_ms: Option<u64>,
    pending_unattributed_text: String,
}

/// Validate the owner prefix row by row. A later foreign cluster must not
/// invalidate earlier compatible owner speech just because the old whole-result
/// recovery required every split cluster to be safe. This uses the same local
/// continuity proof as sequential split recovery, without borrowing the current
/// tail's identity for an earlier audio interval.
fn recover_locally_supported_owner_prefix(
    state: &SyncState,
    provider_result: &Value,
    filtered: &mut SpeakerFilteredResult,
) {
    if !state.local_speaker_tracking_enabled {
        return;
    }
    let Some(target) = state.target_speaker_id.as_deref() else {
        return;
    };
    let Some(phrase) = state.wake_speaker_phrase.as_deref() else {
        return;
    };
    let normalized_phrase = phrase
        .chars()
        .filter(|ch| ch.is_alphanumeric())
        .collect::<String>();
    let Some(rows) = provider_result.get("utterances").and_then(Value::as_array) else {
        return;
    };
    let mut ordered = rows.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|row| utterance_start_ms(row));
    let Some(anchor) = ordered.iter().position(|row| {
        utterance_is_stable(row)
            && utterance_speaker_id(row).as_deref() == Some(target)
            && utterance_contains_normalized_phrase(row, &normalized_phrase)
    }) else {
        return;
    };
    let mut selected = filtered
        .result
        .get("utterances")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if !selected.contains(ordered[anchor]) {
        return;
    }
    let Some(mut previous_end) = utterance_end_ms(ordered[anchor]) else {
        return;
    };
    let mut recovered = false;
    for (offset, row) in ordered[anchor + 1..].iter().enumerate() {
        let Some((start, end)) = utterance_start_ms(row).zip(utterance_end_ms(row)) else {
            break;
        };
        if !utterance_is_stable(row) || start < previous_end || end <= start {
            break;
        }
        if !selected.contains(row) {
            // A same-id exclusion already has stronger owner-absence evidence;
            // only the provider's different-cluster drift is recoverable here.
            if utterance_speaker_id(row)
                .as_deref()
                .map_or(true, |id| id == target)
            {
                break;
            }
            let prefix = serde_json::json!({"utterances": &ordered[..anchor + offset + 2]});
            if provider_has_locally_excluded_later_utterance(state, &prefix)
                || stable_provider_foreign_row_has_local_veto(state, &prefix)
            {
                break;
            }
            let overlapping = state
                .local_speaker_evidence
                .iter()
                .filter(|sample| {
                    let center = sample
                        .audio_end_ms
                        .saturating_sub(LOCAL_SPEAKER_WINDOW_MS / 2);
                    center >= start && center <= end
                })
                .collect::<Vec<_>>();
            if overlapping.is_empty() || overlapping.iter().any(|sample| {
                !sample.stable_target
                    || matches!(
                        sample.classification,
                        crate::speaker_verification::SessionSpeakerClassification::NonTarget { .. }
                    )
            }) || local_non_target_vetoes_split_utterance(state, start, end)
                || local_evidence_confirms_owner_absence_with_verified_wake(
                    row,
                    &state.local_speaker_evidence,
                    Some(phrase),
                    state.local_wake_owner_verified,
                    state.local_speaker_profile_adaptive,
                    state.local_non_target_speech_end_ms,
                )
            {
                break;
            }
            selected.push((*row).clone());
            recovered = true;
        }
        previous_end = end;
    }
    if !recovered {
        return;
    }
    selected.sort_by_key(utterance_start_ms);
    let text = selected
        .iter()
        .filter_map(|row| row.get("text").and_then(Value::as_str))
        .collect::<String>();
    filtered.target_speech_end_ms = selected.iter().filter_map(utterance_end_ms).max();
    filtered.stable_other_speaker_present = ordered.iter().any(|row| {
        utterance_is_stable(row) && utterance_speaker_id(row).is_some() && !selected.contains(row)
    });
    filtered.result["utterances"] = Value::Array(selected);
    filtered.result["text"] = Value::String(text.clone());
    filtered.optimistic_result = filtered.result.clone();
    if !filtered.pending_unattributed_text.is_empty() {
        filtered.pending_unattributed_text = text;
    }
    filtered.response_local_body_alias_present = true;
}

/// A provider cluster id is not a durable person id. A low similarity score
/// alone is not a durable person change either: the owner's own resumed speech
/// can score very low after a pause. Only remove a same-cluster row after the
/// local identity tracker has actually confirmed a departure. The degraded
/// tail signal can corroborate that decision, but cannot make it on its own.
fn degraded_same_cluster_foreign_tail_start(
    rows: &[Value],
    target_speaker_id: Option<&str>,
    evidence: &[LocalSpeakerEvidence],
    wake_owner_verified: bool,
    local_speaker_profile_adaptive: bool,
    degraded_owner_tail: bool,
) -> Option<usize> {
    const BOUNDARY_SLACK_MS: u64 = 250;
    let target = target_speaker_id?;
    if !wake_owner_verified || local_speaker_profile_adaptive || !degraded_owner_tail {
        return None;
    }
    for index in 1..rows.len() {
        let previous = &rows[index - 1];
        let current = &rows[index];
        if !utterance_is_stable(previous)
            || !utterance_is_stable(current)
            || utterance_speaker_id(previous).as_deref() != Some(target)
            || utterance_speaker_id(current).as_deref() != Some(target)
        {
            continue;
        }
        let (Some(previous_end_ms), Some(start_ms), Some(end_ms)) = (
            utterance_end_ms(previous),
            utterance_start_ms(current),
            utterance_end_ms(current),
        ) else {
            continue;
        };
        if start_ms.saturating_add(BOUNDARY_SLACK_MS) < previous_end_ms || end_ms <= start_ms {
            continue;
        }
        let overlapping = evidence.iter().filter(|sample| {
            let center_ms = sample
                .audio_end_ms
                .saturating_sub(LOCAL_SPEAKER_WINDOW_MS / 2);
            center_ms >= start_ms.saturating_sub(BOUNDARY_SLACK_MS)
                && center_ms <= end_ms.saturating_add(BOUNDARY_SLACK_MS)
        });
        if overlapping.into_iter().any(|sample| !sample.stable_target) {
            return Some(index);
        }
    }
    None
}

#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
fn clamp_degraded_same_cluster_foreign_tail(
    state: &SyncState,
    provider_result: &Value,
    filtered: &mut SpeakerFilteredResult,
) {
    let Some(rows) = provider_result.get("utterances").and_then(Value::as_array) else {
        return;
    };
    let mut ordered = rows.clone();
    ordered.sort_by_key(utterance_start_ms);
    let degraded_owner_tail = degraded_owner_tail_suggests_interference_with_quality(
        &state.local_speaker_evidence,
        &state.local_unreliable_speaker_evidence_end_ms,
    );
    let Some(cutoff) = degraded_same_cluster_foreign_tail_start(
        &ordered,
        state.target_speaker_id.as_deref(),
        &state.local_speaker_evidence,
        state.local_wake_owner_verified,
        state.local_speaker_profile_adaptive,
        degraded_owner_tail,
    ) else {
        return;
    };
    let selected = filtered
        .result
        .get("utterances")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let kept = ordered[..cutoff]
        .iter()
        .filter(|row| selected.contains(row))
        .cloned()
        .collect::<Vec<_>>();
    if kept.is_empty() {
        return;
    }
    let text = kept
        .iter()
        .filter_map(|row| row.get("text").and_then(Value::as_str))
        .collect::<String>();
    if text.trim().is_empty() {
        return;
    }
    filtered.target_speech_end_ms = kept.iter().filter_map(utterance_end_ms).max();
    filtered.stable_non_target_utterance_present = true;
    filtered.stable_other_speaker_present = true;
    filtered.pending_unattributed_text.clear();
    filtered.result["utterances"] = Value::Array(kept);
    filtered.result["text"] = Value::String(text.clone());
    filtered.optimistic_result = filtered.result.clone();
    log::info!(
        "[target-speaker] rejected degraded same-cluster foreign tail cutoff_row={} owner_chars={}",
        cutoff,
        text.chars().count()
    );
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

/// The provider leaves a continuing utterance without an end timestamp until
/// its two-pass seal. The ordinary whole-utterance verifier cannot inspect
/// that open row, so a brief stable foreign row can leave all subsequent
/// owner words visual-only for many seconds. Admit an open row belonging to
/// the already anchored cloud owner only after fresh, repeated local Target
/// windows following its start. A local NonTarget or overlap with a sealed
/// foreign row still vetoes the provisional text.
fn local_evidence_allows_open_owner_utterance(
    utterance: &Value,
    target_speaker_id: &str,
    utterances: &[Value],
    evidence: &[LocalSpeakerEvidence],
) -> bool {
    if utterance_is_stable(utterance)
        || utterance_speaker_id(utterance).as_deref() != Some(target_speaker_id)
    {
        return false;
    }
    let Some(start_ms) = utterance_start_ms(utterance) else {
        return false;
    };
    let Some(latest_ms) = evidence.last().map(|sample| sample.audio_end_ms) else {
        return false;
    };
    if latest_ms < start_ms.saturating_add(LOCAL_SPEAKER_WINDOW_MS) {
        return false;
    }
    if utterances.iter().any(|row| {
        utterance_is_stable(row)
            && utterance_speaker_id(row)
                .as_deref()
                .is_some_and(|speaker| speaker != target_speaker_id)
            && utterance_end_ms(row).is_some_and(|end_ms| end_ms > start_ms)
    }) {
        return false;
    }
    let mut target_votes = 0;
    let mut latest_target_ms = None;
    for sample in evidence {
        let center_ms = sample
            .audio_end_ms
            .saturating_sub(LOCAL_SPEAKER_WINDOW_MS / 2);
        if center_ms < start_ms {
            continue;
        }
        match sample.classification {
            crate::speaker_verification::SessionSpeakerClassification::Target { .. }
                if sample.stable_target =>
            {
                target_votes += 1;
                latest_target_ms = Some(sample.audio_end_ms);
            }
            crate::speaker_verification::SessionSpeakerClassification::NonTarget { .. } => {
                return false;
            }
            _ => {}
        }
    }
    target_votes >= 2
        && latest_target_ms.is_some_and(|at| latest_ms.saturating_sub(at) <= LOCAL_SPEAKER_WINDOW_MS)
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

/// A sealed row in the already anchored cloud speaker track can outrun the
/// *centre* of the local verifier's latest 1.2 s window. That part of the row
/// has no local identity verdict yet. Keep the provider's same-speaker
/// attribution for at most one verifier cadence while the last observed
/// identity still belongs to the owner; an observed NonTarget or a stale
/// classifier cannot be converted into owner evidence. Different cloud
/// speaker IDs continue to require a positive overlapping local observation.
fn recent_unobserved_same_cloud_owner_row(
    utterance: &Value,
    evidence: &[LocalSpeakerEvidence],
) -> bool {
    let Some(start_ms) = utterance_start_ms(utterance) else {
        return false;
    };
    let Some(last) = evidence.last() else {
        return false;
    };
    let last_center_ms = last.audio_end_ms.saturating_sub(LOCAL_SPEAKER_WINDOW_MS / 2);
    last_center_ms < start_ms
        && start_ms.saturating_sub(last_center_ms) <= LOCAL_SPEAKER_WINDOW_MS / 2
        && last.stable_target
        && !matches!(
            last.classification,
            crate::speaker_verification::SessionSpeakerClassification::NonTarget { .. }
        )
        && !local_evidence_confirms_owner_absence(utterance, evidence, None)
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
    _wake_speaker_phrase: Option<&str>,
    _wake_owner_verified: bool,
    _local_speaker_profile_adaptive: bool,
    _confirmed_non_target_speech_end_ms: Option<u64>,
) -> bool {
    let Some(end_ms) = utterance_end_ms(utterance) else {
        return false;
    };
    let start_ms = utterance_start_ms(utterance).unwrap_or(end_ms);
    // Same-speaker provider rows are kept through uncertain local windows.
    // A score dip after a pause is not enough to erase recognized owner text.
    // In installed session 350774d8 every provider row was speaker 0 and the
    // local identity remained stable, yet this former low-score vote deleted
    // most of the body and froze streaming after 140 characters were pasted.
    for sample in evidence {
        let sample_center_ms = sample
            .audio_end_ms
            .saturating_sub(LOCAL_SPEAKER_WINDOW_MS / 2);
        if sample_center_ms < start_ms || sample_center_ms > end_ms {
            continue;
        }
        if !sample.stable_target {
            return true;
        }
    }
    false
}

/// Contrastive continuation acceptance for the owner's own later sentences
/// under sustained background media (installed sessions e57053aa / 6eba7dc3 /
/// 2dd35bbf, 2026-09-19 22:07Z). While media plays, the owner's voice mixed
/// into the microphone scores only 0.26-0.44 against the session target
/// (Uncertain), so no absolute band can prove ownership; meanwhile the pure
/// media gap windows of the same session sit at 0.03-0.12 (NonTarget). The
/// RELATIVE margin inside one session is what separates the owner resuming
/// speech from the interference itself: accept a later utterance only when it
/// carries no positive non-owner window of its own and its best window clears
/// the session's strongest confirmed non-owner observation by
/// CONTRASTIVE_CONTINUATION_MARGIN. A real second speaker produces their own
/// NonTarget windows against the same-session target (vetoed here and by the
/// corroborated-foreign-row check; the anchor-era measurements put bystander
/// windows at <= 0.11, deep in the NonTarget band — the historical 0.42-0.58
/// cross-speaker scores were measured against the pre-2026-09-18 TTS-like
/// bank, not against a verified same-session wake anchor). A session with no
/// interference cluster at all falls back to the absolute floor plus duration
/// evidence (>= 2 overlapping windows): the quiet-room installed session
/// 7188c773 showed the owner's own clause can sit entirely in the documented
/// same-speaker 0.30-0.34 cross-phrase dip with no NonTarget window anywhere.
fn local_evidence_contrastive_owner_continuation(
    evidence: &[LocalSpeakerEvidence],
    start_ms: u64,
    end_ms: u64,
) -> bool {
    const CONTRASTIVE_CONTINUATION_MARGIN: f32 = 0.12;
    const CONTRASTIVE_CONTINUATION_MIN_SCORE: f32 = 0.25;
    /// Without a session interference cluster the contrast falls back to the
    /// absolute floor plus duration evidence (>= 2 overlapping windows). The
    /// installed quiet-room session 7188c773 (2026-09-20 08:14) proved the
    /// owner's own short clause ("再整体提交一遍1.0.5呗", sealed as its own
    /// cloud row) can dip to Uncertain with no NonTarget window anywhere in
    /// the session; requiring an interference baseline there amputated the
    /// clause while its neighbours survived.
    const CONTRASTIVE_CONTINUATION_MIN_WINDOWS: u32 = 2;
    let mut room_non_owner_baseline: Option<f32> = None;
    for sample in evidence {
        if matches!(
            sample.classification,
            crate::speaker_verification::SessionSpeakerClassification::NonTarget { .. }
        ) {
            room_non_owner_baseline = Some(
                room_non_owner_baseline
                    .map_or(sample.classification.score(), |baseline: f32| {
                        baseline.max(sample.classification.score())
                    }),
            );
        }
    }
    let room_non_owner_baseline = room_non_owner_baseline.unwrap_or(0.0);
    let mut best_score = 0.0f32;
    let mut overlap_count = 0u32;
    for sample in evidence {
        let sample_center_ms = sample.audio_end_ms.saturating_sub(LOCAL_SPEAKER_WINDOW_MS / 2);
        if sample_center_ms < start_ms || sample_center_ms > end_ms {
            continue;
        }
        overlap_count += 1;
        if matches!(
            sample.classification,
            crate::speaker_verification::SessionSpeakerClassification::NonTarget { .. }
        ) {
            // A positive non-owner window inside the utterance itself is the
            // absolute veto: the owner's own continuation never scores below
            // the room's confirmed non-owner ceiling.
            return false;
        }
        best_score = best_score.max(sample.classification.score());
    }
    overlap_count >= CONTRASTIVE_CONTINUATION_MIN_WINDOWS
        && best_score >= CONTRASTIVE_CONTINUATION_MIN_SCORE
        && best_score >= room_non_owner_baseline + CONTRASTIVE_CONTINUATION_MARGIN
}

/// A phrase-only wake can miss the enrolled bank even though the subsequent
/// body has already produced repeated positive owner matches. Permit the
/// same-cloud-speaker continuation to use those body matches as its anchor.
/// Keep this stricter than a bank-verified wake: a merely Uncertain first
/// clause must not license a later room speaker with a reused cloud id.
fn locally_verified_body_anchor_for_continuation(
    evidence: &[LocalSpeakerEvidence],
    continuation_start_ms: u64,
    continuation_end_ms: u64,
) -> bool {
    const MIN_ANCHOR_SEPARATION_MS: u64 = 400;
    const MIN_CONTINUATION_SCORE: f32 = 0.30;
    const MIN_ANCHOR_RATIO: f32 = 0.70;
    let anchors = evidence
        .iter()
        .filter(|sample| {
            sample.audio_end_ms < continuation_start_ms
                && sample.stable_target
                && matches!(
                    sample.classification,
                    crate::speaker_verification::SessionSpeakerClassification::Target { .. }
                )
        })
        .collect::<Vec<_>>();
    if anchors.len() < 2
        || anchors.last().unwrap().audio_end_ms.saturating_sub(anchors[0].audio_end_ms)
            < MIN_ANCHOR_SEPARATION_MS
    {
        return false;
    }
    let anchor_score = anchors
        .iter()
        .map(|sample| sample.classification.score())
        .fold(0.0f32, f32::max);
    let continuation_score = evidence
        .iter()
        .filter(|sample| {
            let center = sample.audio_end_ms.saturating_sub(LOCAL_SPEAKER_WINDOW_MS / 2);
            center >= continuation_start_ms && center <= continuation_end_ms
        })
        .map(|sample| sample.classification.score())
        .fold(0.0f32, f32::max);
    continuation_score >= MIN_CONTINUATION_SCORE
        && continuation_score >= anchor_score * MIN_ANCHOR_RATIO
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
    state.pause_early_recent_revisions.clear();
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
    state.pause_early_recent_revisions.clear();

    // A provisional preview may already contain the other person's words while
    // diarization was pending. Replace every committed/display ledger with the
    // filtered owner text so protocol-final fallback cannot restore that tail.
    if state.best_transcript_text != candidate.text {
        state.best_transcript_committed_at = Some(Instant::now());
    }
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
        ) || recent_unobserved_same_cloud_owner_row(utterance, local_speaker_evidence);
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
    let cloud_id_matches = utterance_speaker_id(utterance).as_deref() == Some(target_speaker_id);
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
    // A time-aligned local sample is helpful, but not guaranteed for every
    // sealed cloud row. Once the wake owner is verified, a stable row in that
    // same cloud track remains the best available main-body candidate unless
    // the local identity actually departed. Otherwise sampling gaps after a
    // pause silently remove entire recognized sentences.
    let verified_same_cluster_continuation_belongs =
        wake_owner_verified && cloud_id_matches && utterance_is_stable(utterance);
    // Media-time contrastive continuation (e57053aa family, 2026-09-19): the
    // owner's resumed sentence after a thinking pause carries only
    // media-mixed Uncertain windows (0.26-0.44 vs the session target), so both
    // the strict later-split rule and the owner-departure veto below would cut
    // it even though the provider heard every word and no window places a
    // second person inside it. Accept it only on the relative contrast against
    // the session's own interference cluster; see
    // local_evidence_contrastive_owner_continuation for the bystander-safety
    // contract.
    let contrastive_continuation_belongs = (wake_owner_verified
        || (cloud_id_matches
            && matches!(
                (utterance_start_ms(utterance), utterance_end_ms(utterance)),
                (Some(start_ms), Some(end_ms)) if locally_verified_body_anchor_for_continuation(
                    local_speaker_evidence,
                    start_ms,
                    end_ms,
                )
            )))
        && matches!(
            (utterance_start_ms(utterance), utterance_end_ms(utterance)),
            (Some(start_ms), Some(end_ms)) if start_ms
                >= wake_speaker_end_ms.unwrap_or_default().saturating_sub(200)
                && local_evidence_contrastive_owner_continuation(
                    local_speaker_evidence,
                    start_ms,
                    end_ms,
                )
        );
    if !baseline_belongs
        && !verified_wake_anchor_belongs
        && !verified_same_cluster_continuation_belongs
        && !contrastive_continuation_belongs
    {
        return false;
    }
    // Cloud can reuse the owner's speaker id for the next real person. The
    // verified-wake path supplies the missing identity authority, while the
    // sustained per-utterance low-score checks keep ordinary owner dips safe.
    // A contrastively accepted continuation is exactly the owner-dip shape the
    // unstable-window early return below would misread as a departure.
    !(utterance_speaker_id(utterance).as_deref() == Some(target_speaker_id)
        && !contrastive_continuation_belongs
        && local_evidence_confirms_owner_absence_with_verified_wake(
            utterance,
            local_speaker_evidence,
            wake_speaker_phrase,
            wake_owner_verified,
            local_speaker_profile_adaptive,
            confirmed_non_target_speech_end_ms,
        ))
}

fn utterance_belongs_to_verified_target_or_body_alias(
    utterance: &Value,
    target_speaker_id: &str,
    immediate_body_speaker_id: Option<&str>,
    local_speaker_tracking_enabled: bool,
    local_speaker_evidence: &[LocalSpeakerEvidence],
    wake_speaker_phrase: Option<&str>,
    wake_speaker_end_ms: Option<u64>,
    allow_immediate_wake_continuation: bool,
    wake_owner_verified: bool,
    local_speaker_profile_adaptive: bool,
    confirmed_non_target_speech_end_ms: Option<u64>,
    confirmed_foreign_hint_end_ms: Option<u64>,
) -> bool {
    let utterance_speaker = utterance_speaker_id(utterance);
    // A short local mismatch cannot reject same-cluster text. When the cloud
    // independently marks a stable, time-aligned different speaker, however,
    // the two weak signals form one explicit foreign-row veto. Keep this check
    // before applying the response-local body alias.
    let corroborated_foreign_row = utterance_is_stable(utterance)
        && utterance_speaker
            .as_deref()
            .is_some_and(|speaker| speaker != target_speaker_id)
        && confirmed_foreign_hint_end_ms
            .into_iter()
            .chain(
                local_speaker_evidence
                    .iter()
                    .rev()
                    .take(1)
                    .filter_map(|sample| {
                        matches!(sample.classification,
                    crate::speaker_verification::SessionSpeakerClassification::NonTarget { .. }
                ).then_some(sample.audio_end_ms)
                    }),
            )
            .any(|audio_end_ms| {
                utterance_start_ms(utterance)
                    .zip(utterance_end_ms(utterance))
                    .is_some_and(|(start_ms, end_ms)| {
                        let center_ms = audio_end_ms.saturating_sub(LOCAL_SPEAKER_WINDOW_MS / 2);
                        center_ms >= start_ms && center_ms <= end_ms
                    })
            });
    if corroborated_foreign_row {
        return false;
    }
    let body_alias_matches =
        immediate_body_speaker_id.is_some_and(|alias| utterance_speaker.as_deref() == Some(alias));
    let effective_target = if body_alias_matches {
        immediate_body_speaker_id.unwrap_or(target_speaker_id)
    } else {
        target_speaker_id
    };
    utterance_belongs_to_verified_target(
        utterance,
        effective_target,
        local_speaker_tracking_enabled,
        local_speaker_evidence,
        wake_speaker_phrase,
        wake_speaker_end_ms,
        allow_immediate_wake_continuation && !body_alias_matches,
        wake_owner_verified,
        local_speaker_profile_adaptive,
        confirmed_non_target_speech_end_ms,
    )
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
    confirmed_foreign_hint_end_ms: Option<u64>,
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
        // E/F attribution contract: the target anchor must come from an
        // identity source — the cloud row that spoke the wake phrase, or the
        // local speaker-evidence timeline. Never anchor to an anonymous first
        // stable utterance: r17 (work/human-endpoint-cd-r17-20260917) locked
        // the target onto a bystander's leading sentence while local tracking
        // had failed to activate, and the kernel veto then destroyed the
        // owner's whole body. With no identity source the filter abstains
        // (target stays None) so raw recovery can preserve the full text.
        *target_speaker_id = wake_speaker_phrase
            .and_then(|phrase| wake_phrase_bound_target_speaker(&utterances, phrase))
            .or_else(|| {
                local_speaker_tracking_enabled
                    .then(|| locally_bound_target_speaker(&utterances, local_speaker_evidence))
                    .flatten()
            });
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
    // The rolling-response shape needs the continuity exception when the
    // verified wake was present in an earlier packet and this packet contains
    // no stable row for that cluster.
    let allow_immediate_wake_continuation = prior_wake_speaker_end_ms.is_some()
        && target_speaker_id.as_deref().is_some_and(|target| {
            !utterances.iter().any(|utterance| {
                utterance_is_stable(utterance)
                    && utterance_speaker_id(utterance).as_deref() == Some(target)
            })
        });

    // A full final response can contain both the short physical-wake cluster
    // and a new cluster for the same uninterrupted owner body. Keep that new
    // id as a response-local alias only: the wake must have been verified, the
    // boundary must be tightly contiguous, and local identity evidence for the
    // first body utterance must still retain the owner. Subsequent utterances
    // with that same alias are independently checked against local evidence.
    // Never persist the alias, so an ordinary later speaker-id change cannot
    // silently replace the wake anchor in following provider responses.
    let bounded_wake_row_present = wake_speaker_phrase.is_some_and(|phrase| {
        let normalized_phrase = phrase
            .chars()
            .filter(|ch| ch.is_alphanumeric())
            .collect::<String>();
        utterances.iter().any(|utterance| {
            utterance_speaker_id(utterance).as_deref() == target_speaker_id.as_deref()
                && utterance_is_bounded_wake_phrase(utterance, &normalized_phrase)
        })
    });
    let immediate_body_speaker_id = if wake_owner_verified
        && local_speaker_tracking_enabled
        && bounded_wake_row_present
    {
        target_speaker_id.as_deref().and_then(|target| {
            let wake_end_ms = wake_speaker_end_ms?;
            utterances
                .iter()
                .filter(|utterance| utterance_is_stable(utterance))
                .filter_map(|utterance| {
                    let speaker_id = utterance_speaker_id(utterance)?;
                    if speaker_id == target {
                        return None;
                    }
                    let start_ms = utterance_start_ms(utterance)?;
                    (start_ms >= wake_end_ms.saturating_sub(200)
                        && start_ms
                            <= wake_end_ms.saturating_add(MAX_WAKE_BODY_CLUSTER_SPLIT_GAP_MS)
                        && local_evidence_supports_cloud_target(
                            utterance,
                            local_speaker_evidence,
                            wake_speaker_phrase,
                        ))
                    .then_some(speaker_id)
                })
                .next()
        })
    } else {
        None
    };

    let selected = target_speaker_id
        .as_deref()
        .map(|target| {
            utterances
                .iter()
                .filter(|utterance| {
                    utterance_belongs_to_verified_target_or_body_alias(
                        utterance,
                        target,
                        immediate_body_speaker_id.as_deref(),
                        local_speaker_tracking_enabled,
                        local_speaker_evidence,
                        wake_speaker_phrase,
                        wake_speaker_end_ms,
                        allow_immediate_wake_continuation,
                        wake_owner_verified,
                        local_speaker_profile_adaptive,
                        confirmed_non_target_speech_end_ms,
                        confirmed_foreign_hint_end_ms,
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
            let speaker = utterance_speaker_id(utterance);
            utterance_is_stable(utterance)
                && speaker.is_some()
                // A same-speaker row with no local vote is pending identity,
                // not an explicit foreign speaker. Only a local observed
                // departure can make that row a destructive final veto.
                && (speaker.as_deref() != Some(target)
                    || local_evidence_confirms_owner_absence_with_verified_wake(
                        utterance,
                        local_speaker_evidence,
                        wake_speaker_phrase,
                        wake_owner_verified,
                        local_speaker_profile_adaptive,
                        confirmed_non_target_speech_end_ms,
                    ))
                && !utterance_belongs_to_verified_target_or_body_alias(
                    utterance,
                    target,
                    immediate_body_speaker_id.as_deref(),
                    local_speaker_tracking_enabled,
                    local_speaker_evidence,
                    wake_speaker_phrase,
                    wake_speaker_end_ms,
                    allow_immediate_wake_continuation,
                    wake_owner_verified,
                    local_speaker_profile_adaptive,
                    confirmed_non_target_speech_end_ms,
                    confirmed_foreign_hint_end_ms,
                )
        })
    });
    // Filtering can abstain when a stable row is outside local analysis
    // coverage. Preserve that third state: it is neither an accepted owner
    // row nor an explicit foreign row, and cannot authorize raw-final recovery.
    let stable_unresolved_speaker_present = target_speaker_id.as_deref().is_some_and(|target| {
        utterances.iter().any(|utterance| {
            utterance_is_stable(utterance)
                && utterance_speaker_id(utterance).as_deref() == Some(target)
                && !selected.contains(utterance)
                && !local_evidence_confirms_owner_absence_with_verified_wake(
                    utterance,
                    local_speaker_evidence,
                    wake_speaker_phrase,
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
                utterance_belongs_to_verified_target_or_body_alias(
                    utterance,
                    target,
                    immediate_body_speaker_id.as_deref(),
                    local_speaker_tracking_enabled,
                    local_speaker_evidence,
                    wake_speaker_phrase,
                    wake_speaker_end_ms,
                    allow_immediate_wake_continuation,
                    wake_owner_verified,
                    local_speaker_profile_adaptive,
                    confirmed_non_target_speech_end_ms,
                    confirmed_foreign_hint_end_ms,
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
                || target_speaker_id.as_deref().is_some_and(|target| {
                    local_evidence_allows_open_owner_utterance(
                        utterance,
                        target,
                        &utterances,
                        local_speaker_evidence,
                    )
                })
        })
        .cloned()
        .collect::<Vec<_>>();
    let optimistic_utterance_text = optimistic_utterances
        .iter()
        .filter_map(|utterance| utterance.get("text").and_then(Value::as_str))
        .collect::<String>();
    // A two-pass row can already contain the live tail while an overlapping
    // stream row still repeats that same tail. Joining the rows then makes the
    // optimistic text longer than the provider's cumulative result and can
    // paste the repeated words before the final correction arrives. Use the
    // provider's complete wording when the only extra row text is an exact
    // repeat of its own ending; do not collapse a genuinely new utterance.
    let overlapping_last_row = optimistic_utterances
        .split_last()
        .and_then(|(last, previous)| {
            utterance_start_ms(last).zip(previous.iter().filter_map(utterance_end_ms).max())
        })
        .is_some_and(|(start, previous_end)| start < previous_end);
    let optimistic_utterance_text = if !target_text.is_empty()
        && !stable_other_speaker_present
        && overlapping_last_row
        && raw_text.starts_with(&target_text)
        && optimistic_utterance_text
            .strip_prefix(raw_text)
            .is_some_and(|extra| {
                extra.chars().count() >= 2
                    && raw_text.ends_with(extra)
                    && optimistic_utterances.last().and_then(|row| row.get("text"))
                        .and_then(Value::as_str)
                        == Some(extra)
            })
    {
        log::info!(
            "[asr] reconciled overlapping stream row with provider cumulative text provider_chars={} joined_chars={}",
            raw_text.chars().count(),
            optimistic_utterance_text.chars().count()
        );
        raw_text.to_string()
    } else {
        optimistic_utterance_text
    };
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
        response_local_body_alias_present: immediate_body_speaker_id.is_some(),
        stable_non_target_utterance_present: stable_same_cluster_owner_absence_present,
        stable_other_speaker_present,
        stable_unresolved_speaker_present,
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
    asr_stream_id: AtomicU64,
    credentials: VolcengineCredentials,
    hotwords: Vec<DictionaryHotword>,
    session_options: VolcengineSessionOptions,
    proxy_config: ProviderProxyConfig,
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
    /// Serializes observation registration with enqueue and terminal cleanup.
    /// This is deliberately separate from the audio state lock: it protects
    /// the bookkeeping boundary without making observation state control PCM
    /// admission or transport delivery.
    delivery_queue_lock: ParkingMutex<()>,
    /// 队列里 + worker 在飞的 audio 帧总数。consume +N，worker send 完一帧 -1。
    /// send_last_frame 必须等它降到 0 才能安全发末帧，否则末帧可能被服务端先收到
    /// 而把后续 chunk 当成「stream 已结束」之后的多余数据丢弃 → 尾句丢失。
    pending_sends: Arc<AtomicUsize>,
    /// Session 内排队或正在写入 WebSocket 的音频帧峰值，用于验证消费者是否持续跟上
    /// 100 ms 一帧的生产速度；不记录任何音频内容。
    pending_sends_high_water: Arc<AtomicUsize>,
    send_done: Arc<Notify>,
    audio_delivery_changed: Arc<Notify>,
    pipeline_observation:
        ParkingMutex<Option<Arc<crate::observability::EmbeddedAudioPipelineObservation>>>,
    queued_audio_observations: ParkingMutex<HashMap<(u64, i32), Vec<AudioSourceShare>>>,
    in_flight_audio_observations: ParkingMutex<HashMap<(u64, i32), Vec<AudioSourceShare>>>,
    final_frame_result: OnceCell<Result<(), VolcengineASRError>>,
    #[cfg(test)]
    test_final_wait_barrier: ParkingMutex<Option<Arc<TestFinalWaitBarrier>>>,
    /// Full normalized session audio retained on the host for one exceptional
    /// replay when the live WebSocket transport fails. At 32 KiB/s this stays
    /// small for dictation sessions and does not affect device memory.
    retained_pcm: ParkingMutex<Vec<u8>>,
    recovery_replay_started: AtomicBool,
    diagnostic_trace_enabled: bool,
    diagnostic_trace: ParkingMutex<Vec<VolcengineDiagnosticTraceEntry>>,
    diagnostic_trace_entries_dropped: AtomicUsize,
    /// Audio that reached the transport worker without a source registry entry.
    /// Keep this diagnostic on the ASR instance: attributing it to the current
    /// capture would make a cross-session bookkeeping bug look like valid data.
    unassociated_audio_frame_count: AtomicUsize,
    unassociated_audio_pcm_bytes: AtomicUsize,
    /// Evidence-only state for the single-flight local owner classifier.  The
    /// ASR layer never decides how long this may block endpointing; that bound
    /// belongs to the unified owner endpoint controller.
    local_speaker_analysis_pending: AtomicBool,
    /// Independent local VAD evidence. It is kept outside `SyncState` so a
    /// provider transport recovery cannot accidentally reset the VAD stream or
    /// make a stale provider snapshot look like fresh microphone speech.
    local_speech_activity: Arc<ParkingMutex<LocalSpeechEvidence>>,
    /// Exact local PCM edge paired with the VAD sample clock. `SyncState` keeps
    /// millisecond telemetry for legacy callers; this atomic prevents a
    /// provider timestamp from widening the local VAD coverage claim.
    local_audio_samples: AtomicU64,
    #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
    target_speaker_stream:
        ParkingMutex<Option<Arc<super::target_speaker_extraction::TargetSpeakerStream>>>,
    #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
    target_speaker_filter_required: AtomicBool,
    /// r36：最后一次终判封存的 authority 标签（None=尚未封存）。供协调层
    /// 判断 provider_primary 是否为 speaker_filtered 认证稿。
    last_final_seal_authority_label: ParkingMutex<Option<&'static str>>,
    /// r29（2026-09-18，用户反馈"自动结束后处理时间有点长"）：停说到上屏 4.4s =
    /// 主轨终稿 2.5s + 分离轨终稿 2.6s **串行**。分离轨的 finish 只依赖停说前的
    /// 同一段音频，与主轨终稿无数据依赖——把它的启动提前到停说瞬间并行跑，
    /// await_target_speaker_final 改为 join 这个已启动的任务。跳过分支（无干扰
    /// 证据/主轨终稿已切尾）保持原判定点（主轨终稿之后），只是结果改为丢弃而
    /// 不是避免计算，产物语义不变。
    #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
    target_speaker_final_task: ParkingMutex<
        Option<(
            Arc<super::target_speaker_extraction::TargetSpeakerStream>,
            tauri::async_runtime::JoinHandle<
                Result<Option<crate::asr::RawTranscript>, String>,
            >,
        )>,
    >,
}

#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
fn target_speaker_final_required(
    physical_interference_detected: bool,
    sustained_non_target_seen: bool,
    degraded_owner_tail_seen: bool,
) -> bool {
    crate::speech_decision_kernel::decide_interference(
        crate::speech_decision_kernel::InterferenceEvidence {
            physical_overlap: physical_interference_detected,
            sustained_non_target: sustained_non_target_seen,
            degraded_owner_tail: degraded_owner_tail_seen,
        },
    ) == crate::speech_decision_kernel::InterferenceDecision::AwaitSeparatedOwner
}

#[cfg(test)]
struct TestFinalWaitBarrier {
    entered: Arc<Notify>,
    entered_flag: AtomicBool,
    release: Arc<Notify>,
    final_result: Result<RawTranscript, VolcengineASRError>,
}

#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
fn provider_final_excludes_late_non_target_tail(
    provider_final_received: bool,
    provider_owner_end_ms: Option<u64>,
    local_non_target_end_ms: Option<u64>,
) -> bool {
    provider_final_received
        && provider_owner_end_ms
            .zip(local_non_target_end_ms)
            .is_some_and(|(owner_end_ms, non_target_end_ms)| {
                non_target_end_ms
                    >= owner_end_ms.saturating_add(PROVIDER_CLEAN_NON_TARGET_TAIL_GAP_MS)
            })
}

#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
fn target_speaker_filter_required_after_finish(
    filter_required: bool,
    clean_primary_certified: bool,
) -> bool {
    filter_required && !clean_primary_certified
}

#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
fn target_speaker_filter_required_after_final(
    filter_required: bool,
    explicit_non_owner_tail: bool,
) -> bool {
    filter_required || explicit_non_owner_tail
}

#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
fn recover_incomplete_separator_final_from_distinct_provider_track(
    separator: Result<Option<RawTranscript>, String>,
    provider_target: Option<RawTranscript>,
) -> Result<Option<RawTranscript>, String> {
    let Some(provider_target) = provider_target.filter(|target| !target.text.trim().is_empty())
    else {
        return separator;
    };
    match separator {
        Ok(Some(target)) => {
            let target_chars = spoken_content_len(&target.text);
            let provider_chars = spoken_content_len(&provider_target.text);
            if target_chars == 0
                || target_chars.saturating_mul(5) < provider_chars.saturating_mul(4)
            {
                log::warn!(
                    "[target-speaker] separated final lost owner coverage; using distinct provider target track target_chars={} provider_target_chars={}",
                    target_chars,
                    provider_chars
                );
                Ok(Some(provider_target))
            } else {
                Ok(Some(target))
            }
        }
        Ok(None) => Ok(None),
        Err(err) => {
            log::warn!(
                "[target-speaker] separator final failed but distinct provider target track is available; using provider target: {err}"
            );
            Ok(Some(provider_target))
        }
    }
}

#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
fn recover_noise_only_separator_collapse_from_certified_primary(
    separator: Result<Option<RawTranscript>, String>,
    certified_primary: Option<RawTranscript>,
    interference: crate::speech_decision_kernel::InterferenceEvidence,
) -> Result<Option<RawTranscript>, String> {
    let (Some(primary), Ok(Some(separated))) = (certified_primary, &separator) else {
        return separator;
    };
    let primary_chars = spoken_content_len(&primary.text);
    let separated_chars = spoken_content_len(&separated.text);
    if crate::speech_decision_kernel::certified_primary_rescues_separator_collapse(
        interference,
        primary_chars,
        separated_chars,
    ) {
        log::warn!(
            "[target-speaker] noise-only separated final lost owner coverage; preserving certified primary owner track primary_chars={} separated_chars={}",
            primary_chars,
            separated_chars
        );
        Ok(Some(primary))
    } else {
        separator
    }
}

/// ASR 两轨对同一句话可能把数字写成阿拉伯数字或汉字（r23 实测 "1秒"↔"一秒"）。
/// 对齐时把它们归入同一等价类，避免逐字符编辑距离把等价写法当成内容差异。
#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
fn spoken_char_equivalence_class(ch: char) -> char {
    match ch {
        '0' | '〇' | '零' => '0',
        '1' | '一' => '1',
        '2' | '二' | '两' => '2',
        '3' | '三' => '3',
        '4' | '四' => '4',
        '5' | '五' => '5',
        '6' | '六' => '6',
        '7' | '七' => '7',
        '8' | '八' => '8',
        '9' | '九' => '9',
        _ if ch.is_ascii() => ch.to_ascii_lowercase(),
        _ => ch,
    }
}

/// 把分离轨 final 的说出内容对齐回带标点的说话人过滤 provider 原文，返回原文切片。
///
/// r23 2026-09-18 921fe510（TTS 旁人干扰轮）：产品终稿走 separated_owner 通道，
/// 该文本来自"分离后音频的第二次云解码"——归属内容正确，但没有标点、数字写成
/// 汉字（"1秒"→"一秒"）。既有仲裁只比 spoken_content_len，看不出这种格式损失。
/// 这里以分离轨内容为归属边界：把它作为近似前缀对齐到 provider 说话人过滤原文
/// （带标点、原数字写法），命中则把边界映射回原文下标并切片返回；对不齐时返回
/// None（调用方保持分离轨原文，不引入新文本来源）。边界仍由分离轨决定，provider
/// 原文里未被分离轨覆盖的尾巴（旁人内容）不会进入切片。
#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
fn punctuated_provider_slice_for_separated_final(
    separated: &str,
    provider: &str,
) -> Option<String> {
    const MIN_CONTENT_CHARS: usize = 4;

    // provider 原文中每个"说出字符"（字母/数字）结束处的字节偏移，用于把
    // compact 对齐边界映射回原文下标。
    let mut provider_units = Vec::<(char, usize)>::new();
    for (start, ch) in provider.char_indices() {
        if ch.is_alphanumeric() {
            provider_units.push((spoken_char_equivalence_class(ch), start + ch.len_utf8()));
        }
    }
    let pattern: Vec<char> = separated
        .chars()
        .filter(|ch| ch.is_alphanumeric())
        .map(spoken_char_equivalence_class)
        .collect();
    let text: Vec<char> = provider_units.iter().map(|(ch, _)| *ch).collect();
    if pattern.len() < MIN_CONTENT_CHARS || text.len() < MIN_CONTENT_CHARS {
        return None;
    }

    // 编辑距离 D[i][j] = pattern[..i] 对 text[..j]。取 D[m][j] 最小的 j 作为
    // 分离轨确认的归属边界；并列时取更大的 j（优先覆盖 pattern 对得上的原文）。
    let mut previous: Vec<usize> = (0..=text.len()).collect();
    let mut current = vec![0usize; text.len() + 1];
    for (index, pattern_ch) in pattern.iter().enumerate() {
        current[0] = index + 1;
        for (offset, text_ch) in text.iter().enumerate() {
            let substitution = previous[offset] + usize::from(pattern_ch != text_ch);
            current[offset + 1] = (previous[offset + 1] + 1)
                .min(current[offset] + 1)
                .min(substitution);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    let mut boundary_units = 0usize;
    let mut best_distance = previous[0];
    for (units, distance) in previous.iter().enumerate().skip(1) {
        if *distance <= best_distance {
            best_distance = *distance;
            boundary_units = units;
        }
    }
    let tolerance = (pattern.len() / 8).clamp(1, 8);
    if best_distance > tolerance || boundary_units == 0 {
        return None;
    }

    // 边界后的紧跟标点（句末"。"等）属于该句的终止符，保留；下一个说出字符
    // 之前停止，未确认的内容不进入切片。
    let mut boundary = provider_units[boundary_units - 1].1;
    for ch in provider[boundary..].chars() {
        if ch.is_alphanumeric() {
            break;
        }
        boundary += ch.len_utf8();
    }
    Some(provider[..boundary].trim_end().to_string())
}

/// separated_owner 终稿送入产品仲裁前，用分离轨确认的边界从 provider 说话人
/// 过滤原文恢复带标点的渲染。对不齐（分离轨与 provider 内容确实不同）时保持
/// 分离轨原文（现状行为）。
#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
fn restore_separated_owner_final_punctuation_from_provider_track(
    separator: Result<Option<RawTranscript>, String>,
    provider_target: Option<RawTranscript>,
) -> Result<Option<RawTranscript>, String> {
    let (Some(provider), Ok(Some(separated))) = (&provider_target, &separator) else {
        return separator;
    };
    let Some(restored) =
        punctuated_provider_slice_for_separated_final(&separated.text, &provider.text)
    else {
        return separator;
    };
    if restored == separated.text {
        return separator;
    }
    log::info!(
        "[target-speaker] separated owner final re-rendered from punctuated provider track separated_chars={} restored_chars={} provider_chars={}",
        spoken_content_len(&separated.text),
        spoken_content_len(&restored),
        spoken_content_len(&provider.text)
    );
    Ok(Some(RawTranscript {
        text: restored,
        duration_ms: separated.duration_ms,
    }))
}

#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
#[cfg(test)]
fn degraded_owner_tail_suggests_interference(evidence: &[LocalSpeakerEvidence]) -> bool {
    degraded_owner_tail_suggests_interference_with_quality(evidence, &[])
}

#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
fn degraded_owner_tail_suggests_interference_with_quality(
    evidence: &[LocalSpeakerEvidence],
    unreliable_end_ms: &[u64],
) -> bool {
    const TAIL_WINDOWS: usize = 4;
    // The first two observations still contain the accepted wake phrase. An
    // enrolled wake-phrase embedding naturally scores those windows much
    // higher than arbitrary body speech. Treating that phrase-specific peak as
    // the body baseline made a completely steady owner body look like a late
    // speaker change and synchronously ran the separator during finalization.
    const WAKE_ANCHOR_WINDOWS: usize = 2;
    const MIN_BASELINE_SCORE: f32 = 0.60;
    const REPEATED_DROP: f32 = 0.12;
    const SEVERE_DROP: f32 = 0.18;
    const MIN_DEGRADED_WINDOWS: usize = 3;

    let Some(body_evidence) = evidence.get(WAKE_ANCHOR_WINDOWS..) else {
        return false;
    };
    // Keep the original wake exclusion before filtering. A quiet microphone
    // can produce a low cosine score, but that is not another speaker. The
    // classifier already regards these windows as unreliable; preserve that
    // distinction here instead of forcing a slow separation final on silence.
    let body_evidence = body_evidence
        .iter()
        .filter(|sample| !unreliable_end_ms.contains(&sample.audio_end_ms))
        .collect::<Vec<_>>();
    if body_evidence.len() < TAIL_WINDOWS + 3 {
        return false;
    }
    let tail_start = body_evidence.len() - TAIL_WINDOWS;
    let mut baseline_scores = body_evidence[..tail_start]
        .iter()
        .map(|sample| sample.classification.score())
        .collect::<Vec<_>>();
    baseline_scores.sort_by(f32::total_cmp);
    // Use the upper quartile rather than a single peak. One unusually strong
    // phrase/window must not turn normal cross-phrase score variation into an
    // interference verdict, while a sustained late collapse from a stable
    // body baseline still triggers the separator.
    let reference_index = baseline_scores.len().saturating_sub(1) * 3 / 4;
    let baseline_reference = baseline_scores[reference_index];
    if baseline_reference < MIN_BASELINE_SCORE {
        return false;
    }
    let tail = &body_evidence[tail_start..];
    let degraded = tail
        .iter()
        .filter(|sample| sample.classification.score() + REPEATED_DROP <= baseline_reference)
        .count();
    let severe = tail
        .iter()
        .any(|sample| sample.classification.score() + SEVERE_DROP <= baseline_reference);
    degraded >= MIN_DEGRADED_WINDOWS && severe
}

#[derive(Clone)]
struct RecoverySpeakerSnapshot {
    owner_continuity: OwnerContinuitySnapshot,
    wake_target_speech_end_ms: Option<u64>,
    owner_isolation_frozen: bool,
    owner_isolation_ceiling_text: String,
    owner_isolation_ceiling_segments: Vec<TranscriptSegment>,
}

impl VolcengineStreamingASR {
    pub fn new(credentials: VolcengineCredentials, hotwords: Vec<DictionaryHotword>) -> Self {
        Self::new_with_session_options_and_proxy(
            credentials,
            hotwords,
            VolcengineSessionOptions::default(),
            ProviderProxyConfig::provider_default("volcengine"),
        )
    }

    pub fn new_with_session_options(
        credentials: VolcengineCredentials,
        hotwords: Vec<DictionaryHotword>,
        session_options: VolcengineSessionOptions,
    ) -> Self {
        Self::new_with_session_options_and_proxy(
            credentials,
            hotwords,
            session_options,
            ProviderProxyConfig::provider_default("volcengine"),
        )
    }

    pub fn new_with_proxy_config(
        credentials: VolcengineCredentials,
        hotwords: Vec<DictionaryHotword>,
        proxy_config: ProviderProxyConfig,
    ) -> Self {
        Self::new_with_session_options_and_proxy(
            credentials,
            hotwords,
            VolcengineSessionOptions::default(),
            proxy_config,
        )
    }

    pub fn new_with_session_options_and_proxy(
        credentials: VolcengineCredentials,
        hotwords: Vec<DictionaryHotword>,
        session_options: VolcengineSessionOptions,
        proxy_config: ProviderProxyConfig,
    ) -> Self {
        Self {
            asr_stream_id: AtomicU64::new(next_volcengine_asr_stream_id()),
            credentials,
            hotwords,
            session_options,
            proxy_config,
            state: ParkingMutex::new(SyncState::default()),
            partial_callback: ParkingMutex::new(None),
            visual_partial_callback: ParkingMutex::new(None),
            final_intermediate_callback: ParkingMutex::new(None),
            target_speaker_update_callback: ParkingMutex::new(None),
            streaming_event_callback: ParkingMutex::new(None),
            writer: Arc::new(AsyncMutex::new(None)),
            final_rx: ParkingMutex::new(None),
            audio_tx: ParkingMutex::new(None),
            delivery_queue_lock: ParkingMutex::new(()),
            pending_sends: Arc::new(AtomicUsize::new(0)),
            pending_sends_high_water: Arc::new(AtomicUsize::new(0)),
            send_done: Arc::new(Notify::new()),
            audio_delivery_changed: Arc::new(Notify::new()),
            pipeline_observation: ParkingMutex::new(None),
            queued_audio_observations: ParkingMutex::new(HashMap::new()),
            in_flight_audio_observations: ParkingMutex::new(HashMap::new()),
            final_frame_result: OnceCell::new(),
            #[cfg(test)]
            test_final_wait_barrier: ParkingMutex::new(None),
            retained_pcm: ParkingMutex::new(Vec::new()),
            recovery_replay_started: AtomicBool::new(false),
            diagnostic_trace_enabled: false,
            diagnostic_trace: ParkingMutex::new(Vec::new()),
            diagnostic_trace_entries_dropped: AtomicUsize::new(0),
            unassociated_audio_frame_count: AtomicUsize::new(0),
            unassociated_audio_pcm_bytes: AtomicUsize::new(0),
            local_speaker_analysis_pending: AtomicBool::new(false),
            local_speech_activity: Arc::new(ParkingMutex::new(LocalSpeechEvidence::default())),
            local_audio_samples: AtomicU64::new(0),
            #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
            target_speaker_stream: ParkingMutex::new(None),
            #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
            target_speaker_filter_required: AtomicBool::new(false),
            last_final_seal_authority_label: ParkingMutex::new(None),
            #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
            target_speaker_final_task: ParkingMutex::new(None),
        }
    }

    pub fn enable_diagnostic_trace(&mut self) {
        self.diagnostic_trace_enabled = true;
    }

    pub fn take_diagnostic_trace(&self) -> VolcengineDiagnosticTrace {
        VolcengineDiagnosticTrace {
            entries: std::mem::take(&mut *self.diagnostic_trace.lock()),
            entries_dropped: self
                .diagnostic_trace_entries_dropped
                .swap(0, Ordering::Relaxed),
            unassociated_audio_frame_count: self
                .unassociated_audio_frame_count
                .swap(0, Ordering::Relaxed),
            unassociated_audio_pcm_bytes: self
                .unassociated_audio_pcm_bytes
                .swap(0, Ordering::Relaxed),
        }
    }

    fn record_unassociated_audio(&self, bytes: usize) {
        if bytes == 0 {
            return;
        }
        self.unassociated_audio_frame_count
            .fetch_add(1, Ordering::Relaxed);
        self.unassociated_audio_pcm_bytes
            .fetch_add(bytes, Ordering::Relaxed);
        self.record_diagnostic_trace(
            0,
            false,
            "audio_source_unassociated",
            &format!("pcm_bytes={bytes}"),
        );
    }

    fn record_diagnostic_trace(
        &self,
        frame: usize,
        final_frame: bool,
        stage: &'static str,
        text: &str,
    ) {
        if !self.diagnostic_trace_enabled {
            return;
        }
        let elapsed_ms = self
            .state
            .lock()
            .start
            .map(|s| s.elapsed().as_millis())
            .unwrap_or(0);
        let mut trace = self.diagnostic_trace.lock();
        if trace.len() >= DIAGNOSTIC_TRACE_MAX_ENTRIES {
            self.diagnostic_trace_entries_dropped
                .fetch_add(1, Ordering::Relaxed);
            return;
        }
        let text_truncated = text.chars().count() > DIAGNOSTIC_TRACE_MAX_CHARS;
        trace.push(VolcengineDiagnosticTraceEntry {
            frame,
            elapsed_ms,
            final_frame,
            stage,
            text: text.chars().take(DIAGNOSTIC_TRACE_MAX_CHARS).collect(),
            text_truncated,
        });
    }

    #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
    pub fn start_target_speaker_extraction(
        &self,
        wake_phrase: &str,
        wake_pcm: &[u8],
        wake_end_seconds: f32,
    ) {
        let embedding = match crate::speaker_verification::target_speaker_embedding_for_phrase(
            wake_phrase,
        ) {
            Ok(Some(embedding)) => embedding,
            Ok(None) => {
                let enrollment = super::target_speaker_extraction::wake_phrase_enrollment_pcm(
                    wake_pcm,
                    wake_end_seconds,
                );
                match super::target_speaker_extraction::speaker_embedding_from_enrollment_pcm(
                    &enrollment,
                ) {
                    Ok(embedding) => {
                        log::info!(
                            "[target-speaker] WeSep enrollment encoded from this wake pcm_ms={}",
                            enrollment.len() / 32
                        );
                        embedding
                    }
                    Err(err) => {
                        log::warn!(
                            "[target-speaker] owner-only stream skipped: could not encode wake enrollment: {err}"
                        );
                        return;
                    }
                }
            }
            Err(err) => {
                log::warn!(
                    "[target-speaker] owner-only stream skipped: persistent enrollment unavailable: {err}"
                );
                return;
            }
        };
        self.start_target_speaker_extraction_with_embedding(embedding);
    }

    #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
    fn start_target_speaker_extraction_with_embedding(&self, embedding: Vec<f32>) {
        let mut slot = self.target_speaker_stream.lock();
        if slot.is_some() {
            return;
        }
        *slot = Some(
            super::target_speaker_extraction::TargetSpeakerStream::start(
                self.credentials.clone(),
                self.hotwords.clone(),
                embedding,
                self.proxy_config.clone(),
            ),
        );
        log::info!(
            "[target-speaker] owner-only stream armed encoder_sha256={} separator_sha256={}",
            super::target_speaker_extraction::ENCODER_MODEL_SHA256,
            super::target_speaker_extraction::SEPARATOR_MODEL_SHA256
        );
    }

    #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
    pub fn target_speaker_filter_was_required(&self) -> bool {
        self.target_speaker_filter_required.load(Ordering::SeqCst)
    }

    /// r36：主轨终稿是否以 speaker_filtered 认证封存（说话人过滤已切尾的
    /// 干净本人全文）。产品仲裁据此区分"分离稿在做排除"与"分离稿截断丢正文"。
    #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
    pub fn primary_seal_speaker_filtered_certified(&self) -> bool {
        self.last_final_seal_authority_label
            .lock()
            .is_some_and(|label| label == "speaker_filtered")
    }

    #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
    /// Start the independent separator final after all trailing device PCM
    /// has been delivered, overlapping its tail inference with the primary
    /// provider final. The primary final still decides whether to use it.
    pub fn begin_target_speaker_final_early(&self) {
        // A clean session does not need its final separator chunk. Do not
        // compete with the primary provider unless capture has already shown
        // owner/foreign ambiguity. Evidence discovered in the trailing chunk
        // still takes the normal serial path after the primary final.
        let evidence_seen = {
            let state = self.state.lock();
            state.local_sustained_non_target_speech_end_ms.is_some()
                || degraded_owner_tail_suggests_interference_with_quality(
                    &state.local_speaker_evidence,
                    &state.local_unreliable_speaker_evidence_end_ms,
                )
        };
        let physical_overlap = self
            .target_speaker_stream
            .lock()
            .as_ref()
            .is_some_and(|stream| stream.interference_detected());
        if !evidence_seen && !physical_overlap {
            return;
        }
        let stream = self.target_speaker_stream.lock().take();
        let Some(stream) = stream else {
            return;
        };
        let task =
            tauri::async_runtime::spawn({
                let stream = Arc::clone(&stream);
                async move { stream.finish().await }
            });
        log::info!(
            "[target-speaker] parallel final started after primary audio drain physical_overlap={physical_overlap} local_interference={evidence_seen}"
        );
        let mut slot = self.target_speaker_final_task.lock();
        if let Some((_, previous)) = slot.replace((stream, task)) {
            previous.abort();
        }
    }

    pub async fn await_target_speaker_final(&self) -> Result<Option<RawTranscript>, String> {
        // r29 延迟优化：早启动时 finish 已在跑，这里保留流的 Arc 句柄供证据读取
        // 与取消使用，结果从后台任务 join；未早启动则保持原串行语义。
        // 跳过分支（无干扰证据/主轨终稿已切尾）判定点不变（主轨终稿之后），
        // 早启动场景下取消 = 尽早掐掉分离 ASR 并丢弃其结果，产物语义不变。
        let early = self.target_speaker_final_task.lock().take();
        let (early_task, stream) = match early {
            Some((stream, task)) => (Some(task), Some(stream)),
            None => (None, self.target_speaker_stream.lock().take()),
        };
        let Some(stream) = stream else {
            return Ok(None);
        };
        let (
            sustained_non_target_seen,
            degraded_owner_tail_seen,
            provider_final_received,
            provider_owner_end_ms,
            local_non_target_end_ms,
        ) = {
            let state = self.state.lock();
            (
                state.local_sustained_non_target_speech_end_ms.is_some(),
                degraded_owner_tail_suggests_interference_with_quality(
                    &state.local_speaker_evidence,
                    &state.local_unreliable_speaker_evidence_end_ms,
                ),
                state.final_tx.is_none(),
                state.target_speech_end_ms,
                state.local_sustained_non_target_speech_end_ms,
            )
        };
        let physical_interference_detected = stream.interference_detected();
        let interference = crate::speech_decision_kernel::InterferenceEvidence {
            physical_overlap: physical_interference_detected,
            sustained_non_target: sustained_non_target_seen,
            degraded_owner_tail: degraded_owner_tail_seen,
        };
        if !target_speaker_final_required(
            physical_interference_detected,
            sustained_non_target_seen,
            degraded_owner_tail_seen,
        ) {
            stream.cancel();
            if let Some(task) = early_task.as_ref() {
                task.abort();
            }
            log::info!(
                "[target-speaker] no explicit non-owner identity evidence; preserving low-latency certified primary final physical_overlap={physical_interference_detected}"
            );
            return Ok(None);
        }
        if !physical_interference_detected
            && provider_final_excludes_late_non_target_tail(
                provider_final_received,
                provider_owner_end_ms,
                local_non_target_end_ms,
            )
        {
            stream.cancel();
            if let Some(task) = early_task.as_ref() {
                task.abort();
            }
            log::info!(
                "[target-speaker] provider final ended before the confirmed non-target tail; skipping redundant owner-only wait provider_owner_end_ms={provider_owner_end_ms:?} local_non_target_end_ms={local_non_target_end_ms:?}"
            );
            return Ok(None);
        }
        self.target_speaker_filter_required
            .store(true, Ordering::SeqCst);
        log::info!(
            "[target-speaker] awaiting owner-only final physical_overlap={} sustained_non_target={} degraded_owner_tail={}",
            physical_interference_detected,
            sustained_non_target_seen,
            degraded_owner_tail_seen
        );
        let mut early_task = early_task;
        let finish_fut = async {
            match early_task.as_mut() {
                Some(task) => task
                    .await
                    .map_err(|err| format!("target-speaker early finish join failed: {err}"))
                    .and_then(|result| result),
                None => stream.finish().await,
            }
        };
        match tokio::time::timeout(TARGET_SPEAKER_FINAL_WAIT, finish_fut).await {
            Ok(result) => {
                // `TargetSpeakerStream::finish()` returns `Ok(None)` only when
                // the separator measured a clean owner stream and deliberately
                // kept the low-latency primary ASR. A degraded tail score can
                // trigger this final check in an otherwise clean session; do
                // not leave `filter_required` armed and erase that certified
                // primary transcript in the coordinator.
                let clean_primary_certified = matches!(&result, Ok(None));
                let filter_required =
                    target_speaker_filter_required_after_finish(true, clean_primary_certified);
                self.target_speaker_filter_required
                    .store(filter_required, Ordering::SeqCst);
                if clean_primary_certified {
                    log::info!(
                        "[target-speaker] clean primary certified by separator; preserving primary final"
                    );
                }
                let (distinct_provider_target, certified_primary) = {
                    let state = self.state.lock();
                    let duration_ms = state
                        .start
                        .map(|start| start.elapsed().as_millis() as u64)
                        .unwrap_or_default();
                    (
                        state
                            .distinct_speaker_target_final_text
                            .clone()
                            .map(|text| RawTranscript { text, duration_ms }),
                        state
                            .wake_bound_single_speaker_final_text
                            .clone()
                            .map(|text| RawTranscript { text, duration_ms }),
                    )
                };
                let recovered = recover_incomplete_separator_final_from_distinct_provider_track(
                    result,
                    distinct_provider_target.clone(),
                );
                let recovered = recover_noise_only_separator_collapse_from_certified_primary(
                    recovered,
                    certified_primary.clone(),
                    interference,
                );
                // 分离轨只决定归属边界；上屏字符一律取 provider 说话人过滤原文的
                // 带标点切片，避免"分离后二次解码"的格式损失进入终稿。
                restore_separated_owner_final_punctuation_from_provider_track(
                    recovered,
                    distinct_provider_target.or(certified_primary),
                )
            }
            Err(_) => {
                stream.cancel();
                if let Some(task) = early_task.as_ref() {
                    task.abort();
                }
                Err("target-speaker owner-only stream timed out".to_string())
            }
        }
    }

    fn recovery_speaker_snapshot(&self) -> RecoverySpeakerSnapshot {
        let state = self.state.lock();
        RecoverySpeakerSnapshot {
            owner_continuity: OwnerContinuitySnapshot::capture(&state),
            wake_target_speech_end_ms: state.wake_target_speech_end_ms,
            owner_isolation_frozen: state.owner_isolation_frozen,
            owner_isolation_ceiling_text: state.owner_isolation_ceiling_text.clone(),
            owner_isolation_ceiling_segments: state.owner_isolation_ceiling_segments.clone(),
        }
    }

    fn restore_recovery_speaker_snapshot(&self, snapshot: RecoverySpeakerSnapshot) {
        let mut state = self.state.lock();
        snapshot.owner_continuity.restore(&mut state);
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
        let replay = Arc::new(Self::new_with_session_options_and_proxy(
            self.credentials.clone(),
            self.hotwords.clone(),
            self.session_options,
            self.proxy_config.clone(),
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

    #[cfg(test)]
    pub(crate) fn recovery_replay_started_for_test(&self) -> bool {
        self.recovery_replay_started.load(Ordering::SeqCst)
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

    /// The replay deadline includes its queued-audio drain, connection and
    /// provider-final waits. A fixed 15-second coordinator timeout can expire
    /// before a healthy socket sends a longer recording's queued frames.
    pub fn full_audio_replay_timeout(&self) -> Duration {
        let pcm_bytes = self.retained_pcm.lock().len();
        let frame_bytes = AUDIO_FRAME_DURATION_MS as usize * BYTES_PER_MS as usize;
        let queued_frames = pcm_bytes.div_ceil(frame_bytes);
        final_audio_drain_budget(queued_frames)
            + WEBSOCKET_CONNECT_TIMEOUT
            + FINAL_RESULT_TIMEOUT
            + FINAL_FRAME_SEND_BUDGET
            + Duration::from_secs(5)
    }

    pub fn diagnostic_audio_delivery(&self) -> Option<Value> {
        if !self.diagnostic_trace_enabled {
            return None;
        }
        let retained_bytes = self.retained_pcm.lock().len();
        let state = self.state.lock();
        Some(json!({
            "retainedPcmBytes": retained_bytes,
            "queuedCapturedPcmBytes": state.bytes_sent,
            "queuedCapturedFrames": state.frames_sent,
            "providerAudioDurationMs": state.last_server_audio_duration_ms,
            "unassociatedAudioFrameCount": self
                .unassociated_audio_frame_count
                .load(Ordering::Relaxed),
            "unassociatedAudioPcmBytes": self
                .unassociated_audio_pcm_bytes
                .load(Ordering::Relaxed),
        }))
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

    /// Whether the provider transport has definitively failed for this
    /// session. The embedded recorder uses this only for its local silence
    /// safety fallback; normal endpointing still comes from ASR/speaker
    /// evidence, but a failed WebSocket must not leave the device recording
    /// forever because no provider callback can arrive.
    pub(crate) fn audio_delivery_failed(&self) -> bool {
        matches!(
            self.state.lock().audio_delivery_readiness,
            AudioDeliveryReadiness::Failed(_)
        )
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

    /// Snapshot the local owner clock for session endpointing. This is used
    /// only when the provider transport failed before it could emit a target
    /// row; it keeps the endpoint reducer driven by the same owner evidence
    /// instead of falling back to room energy.
    pub fn endpoint_update_snapshot(&self) -> TargetSpeakerUpdate {
        let state = self.state.lock();
        target_speaker_update_from_state(&state, false, false, false)
    }

    /// Endpoint-only snapshot with the independent local VAD sidecar applied.
    /// The public `TargetSpeakerUpdate` field remains the legacy raw-energy
    /// diagnostic everywhere else; only the manual endpoint watchdog consumes
    /// this conservative overlay.
    pub fn endpoint_update_snapshot_with_local_speech_evidence(&self) -> TargetSpeakerUpdate {
        let (mut update, captured_audio_samples) = {
            let state = self.state.lock();
            let update = target_speaker_update_from_state(&state, false, false, false);
            let captured_audio_samples = self.local_audio_samples.load(Ordering::Acquire);
            let captured_audio_samples = if captured_audio_samples > 0 {
                captured_audio_samples
            } else {
                state
                    .local_audio_duration_ms
                    .or(update.audio_duration_ms)
                    .unwrap_or_else(|| self.local_speech_activity.lock().analyzed_through_ms)
                    .saturating_mul(16)
            };
            (update, captured_audio_samples)
        };
        apply_local_speech_evidence_to_update(
            &mut update,
            *self.local_speech_activity.lock(),
            captured_audio_samples,
        );
        update
    }

    /// Apply the endpoint-only local VAD overlay to a provider callback. The
    /// raw callback fields remain unchanged everywhere else; this method is
    /// used by the same reducer boundary as the watchdog so callback arrival
    /// cannot alternate raw energy and VAD semantics.
    pub fn endpoint_update_with_local_speech_evidence_snapshot(
        &self,
        mut update: TargetSpeakerUpdate,
        evidence: LocalSpeechEvidence,
    ) -> TargetSpeakerUpdate {
        let captured_audio_samples = self.local_audio_samples.load(Ordering::Acquire);
        let captured_audio_samples = if captured_audio_samples > 0 {
            captured_audio_samples
        } else {
            self.state
                .lock()
                .local_audio_duration_ms
                .or(update.audio_duration_ms)
                .unwrap_or(evidence.analyzed_through_ms)
                .saturating_mul(16)
        };
        apply_local_speech_evidence_to_update(
            &mut update,
            evidence,
            captured_audio_samples,
        );
        update
    }

    pub fn endpoint_update_with_local_speech_evidence(
        &self,
        update: TargetSpeakerUpdate,
    ) -> TargetSpeakerUpdate {
        self.endpoint_update_with_local_speech_evidence_snapshot(
            update,
            *self.local_speech_activity.lock(),
        )
    }

    pub fn local_speech_activity_revision(&self) -> u64 {
        self.local_speech_activity.lock().revision
    }

    pub fn local_speech_activity_snapshot(&self) -> LocalSpeechEvidence {
        *self.local_speech_activity.lock()
    }

    #[cfg(target_os = "windows")]
    pub fn local_speech_activity_sink(&self) -> Arc<ParkingMutex<LocalSpeechEvidence>> {
        Arc::clone(&self.local_speech_activity)
    }

    /// Publish the lifecycle of the single-flight local speaker job without
    /// pretending that it is owner activity.  The endpoint controller may
    /// wait for this evidence, but the job itself never renews an owner clock.
    pub fn note_local_speaker_analysis_pending(&self, pending: bool) {
        self.local_speaker_analysis_pending
            .store(pending, Ordering::SeqCst);
    }

    pub fn local_speaker_analysis_pending(&self) -> bool {
        self.local_speaker_analysis_pending.load(Ordering::SeqCst)
    }

    /// Record microphone arrival without interpreting raw energy as human
    /// speech. The independent VAD publishes the speech edge separately.
    pub fn note_local_audio_arrival(&self, audio_duration_ms: u64) {
        let mut state = self.state.lock();
        state.local_audio_duration_ms = Some(
            state
                .local_audio_duration_ms
                .unwrap_or_default()
                .max(audio_duration_ms),
        );
    }

    /// Publish independent local VAD evidence. A lagging/unknown result is
    /// intentionally encoded as an edge at the current captured audio so the
    /// endpoint remains conservative until the analyzer catches up.
    pub fn note_local_speech_activity(&self, evidence: LocalSpeechEvidence) {
        {
            let mut latest = self.local_speech_activity.lock();
            if evidence.revision < latest.revision
                || (evidence.revision == latest.revision
                    && evidence.analyzed_through_samples < latest.analyzed_through_samples)
            {
                return;
            }
            *latest = evidence;
        }

        log::debug!(
            "[asr] local VAD evidence revision={} activity_epoch={} analyzed_through_ms={} analyzed_through_samples={} state={:?} last_speech_end_ms={:?} pending_speech_start_ms={:?} trailing_non_speech_ms={:?}",
            evidence.revision,
            evidence.activity_epoch,
            evidence.analyzed_through_ms,
            evidence.analyzed_through_samples,
            evidence.state,
            evidence.last_detected_speech_end_ms,
            evidence.pending_speech_start_ms,
            evidence.trailing_non_speech_ms,
        );
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
            target_speaker_update_from_state(&state, false, false, false)
        };
        if update.speaker_id.is_some() || update.local_speaker_tracking_enabled {
            self.emit_target_speaker_update(update);
        }
    }

    pub fn note_local_audio_activity_samples(
        &self,
        audio_duration_ms: u64,
        audio_samples: u64,
        speech_detected: bool,
    ) {
        let mut previous = self.local_audio_samples.load(Ordering::Acquire);
        while audio_samples > previous {
            match self.local_audio_samples.compare_exchange_weak(
                previous,
                audio_samples,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(observed) => previous = observed,
            }
        }
        self.note_local_audio_activity(audio_duration_ms, speech_detected);
    }

    pub fn note_local_speaker_classification(
        &self,
        audio_duration_ms: u64,
        classification: crate::speaker_verification::SessionSpeakerClassification,
    ) {
        let transcript_speaker_evidence = match classification {
            crate::speaker_verification::SessionSpeakerClassification::NonTarget { score } => {
                crate::speech_decision_kernel::classify_transcript_speaker_evidence(
                    score, 1_200, 512.0,
                )
            }
            _ => crate::speech_decision_kernel::TranscriptSpeakerEvidence::Inconclusive,
        };
        self.note_local_speaker_observation(
            audio_duration_ms,
            classification,
            transcript_speaker_evidence,
        );
    }

    pub fn note_local_speaker_observation(
        &self,
        audio_duration_ms: u64,
        classification: crate::speaker_verification::SessionSpeakerClassification,
        transcript_speaker_evidence: crate::speech_decision_kernel::TranscriptSpeakerEvidence,
    ) {
        self.note_local_speaker_observation_with_quality(
            audio_duration_ms,
            classification,
            transcript_speaker_evidence,
            true,
        );
    }

    pub fn note_local_speaker_observation_with_quality(
        &self,
        audio_duration_ms: u64,
        classification: crate::speaker_verification::SessionSpeakerClassification,
        transcript_speaker_evidence: crate::speech_decision_kernel::TranscriptSpeakerEvidence,
        signal_quality_sufficient: bool,
    ) {
        let (update, stable_target, effective_classification, endpoint_strong_non_target) = {
            let mut state = self.state.lock();
            observe_owner_handoff_recovery(&mut state, classification);
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
            let transcript_speaker_evidence =
                if state.local_wake_owner_verified && !state.local_speaker_profile_adaptive {
                    transcript_speaker_evidence
                } else {
                    crate::speech_decision_kernel::TranscriptSpeakerEvidence::Inconclusive
                };
            let transcript_hard_non_target_applies = matches!(
                transcript_speaker_evidence,
                crate::speech_decision_kernel::TranscriptSpeakerEvidence::HardNonTarget
            );
            let transcript_foreign_hint_applies = matches!(
                transcript_speaker_evidence,
                crate::speech_decision_kernel::TranscriptSpeakerEvidence::ForeignTailHint
                    | crate::speech_decision_kernel::TranscriptSpeakerEvidence::HardNonTarget
            );
            if transcript_foreign_hint_applies {
                state.local_preview_foreign_hint_end_ms = Some(audio_duration_ms);
                state.local_consecutive_transcript_foreign_hint = state
                    .local_consecutive_transcript_foreign_hint
                    .saturating_add(1);
                if state.local_consecutive_transcript_foreign_hint
                    >= LOCAL_SPEAKER_SWITCH_CONFIRMATIONS
                {
                    state.local_confirmed_transcript_foreign_hint_end_ms = Some(
                        state
                            .local_confirmed_transcript_foreign_hint_end_ms
                            .unwrap_or_default()
                            .max(audio_duration_ms),
                    );
                }
            } else {
                state.local_consecutive_transcript_foreign_hint = 0;
            }
            if transcript_hard_non_target_applies
                && state.local_wake_owner_verified
                && !state.local_speaker_profile_adaptive
            {
                state.local_consecutive_transcript_hard_non_target = state
                    .local_consecutive_transcript_hard_non_target
                    .saturating_add(1);
                if state.local_consecutive_transcript_hard_non_target
                    >= LOCAL_SPEAKER_SWITCH_CONFIRMATIONS
                {
                    state.local_confirmed_transcript_non_target_end_ms = Some(
                        state
                            .local_confirmed_transcript_non_target_end_ms
                            .unwrap_or_default()
                            .max(audio_duration_ms),
                    );
                    if state.local_consecutive_transcript_hard_non_target
                        == LOCAL_SPEAKER_SWITCH_CONFIRMATIONS
                    {
                        log::info!(
                            "[asr] confirmed extreme local mismatch kept non-destructive pending provider utterance boundary audio_end_ms={audio_duration_ms}"
                        );
                    }
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
            let observation_is_new = state
                .local_speaker_observation_end_ms
                .is_none_or(|previous_ms| audio_duration_ms >= previous_ms);
            let qualified_owner_observation = observation_is_new
                && signal_quality_sufficient
                && stable_target
                && state.local_target_confirmed
                && !state.local_owner_handoff_suspected
                && matches!(
                    effective_classification,
                    crate::speaker_verification::SessionSpeakerClassification::Target { .. }
                );
            let qualified_owner_activity_advanced = qualified_owner_observation
                && state
                    .qualified_owner_speech_end_ms
                    .is_none_or(|previous_ms| audio_duration_ms > previous_ms);
            if qualified_owner_observation {
                state.qualified_owner_speech_end_ms = Some(
                    state
                        .qualified_owner_speech_end_ms
                        .unwrap_or_default()
                        .max(audio_duration_ms),
                );
            }
            // Endpoint clock is owner-only.
            // - Target（确信，≥0.40，2026-09-23 矩阵重校）：本人身份成立时才
            //   刷新。0.55 旧线是 TTS 银行时代校准——本人锚点档案实测 0.42–0.59，
            //   旧线让本人永远 Uncertain（71f64ac1 冻结永不解除的根因）。
            // - Uncertain（0.20–0.40）：不刷新也不冻结。旧世界 0.426–0.58 的
            //   "第二个人"是 pre-2026-09-18 银行测量（anchor-era 旁人 <=0.11，
            //   2026-09-23 真实旁人 <=0.28）；本人说话中途变轻的覆盖由预览增长
            //   刷新兜底（refresh_local_target_from_owner_preview_activity）。
            // - NonTarget: do NOT refresh. Confident other-speaker windows must not
            //   lengthen auto-end while hysteresis still lags the true switch.
            // Growing Target-filtered ASR preview also refreshes the clock via
            // refresh_local_target_from_owner_preview_activity.
            if stable_target
                && state.local_target_confirmed
                && !state.local_owner_handoff_suspected
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
            if observation_is_new {
                state.local_speaker_signal_quality_sufficient = Some(signal_quality_sufficient);
                state.local_speaker_observation_end_ms = Some(audio_duration_ms);
            }
            // Provider revisions re-attribute all earlier rows. Keep this
            // session's time-aligned windows until reset; the old 64-window
            // ring made already accepted clauses vanish from long finals.
            state.local_speaker_evidence.push(LocalSpeakerEvidence {
                audio_end_ms: audio_duration_ms,
                classification: effective_classification,
                stable_target,
            });
            state
                .local_unreliable_speaker_evidence_end_ms
                .retain(|end| *end != audio_duration_ms);
            if !signal_quality_sufficient {
                state
                    .local_unreliable_speaker_evidence_end_ms
                    .push(audio_duration_ms);
            }
            (
                target_speaker_update_from_state(
                    &state,
                    false,
                    false,
                    qualified_owner_activity_advanced,
                ),
                stable_target,
                effective_classification,
                endpoint_strong_non_target,
            )
        };
        log::info!(
            "[asr] local session-speaker evidence classification={classification:?} effective={effective_classification:?} stable_target={stable_target} endpoint_strong_non_target={endpoint_strong_non_target} transcript_speaker_evidence={} audio_end_ms={audio_duration_ms} signal_quality_sufficient={signal_quality_sufficient}",
            transcript_speaker_evidence.label()
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
        state.local_confirmed_transcript_non_target_end_ms = None;
        state.local_consecutive_transcript_foreign_hint = 0;
        state.local_confirmed_transcript_foreign_hint_end_ms = None;
        state.local_preview_foreign_hint_end_ms = None;
        state.local_owner_handoff_suspected = false;
        state.local_owner_handoff_recovery_targets = 0;
        state.local_consecutive_strong_non_target = 0;
        state.local_owner_absence_run_started_ms = None;
        state.local_owner_absence_run_confirmed = false;
        state.local_sustained_non_target_speech_end_ms = None;
        state.local_speaker_evidence.clear();
        state.local_unreliable_speaker_evidence_end_ms.clear();
        state.qualified_owner_speech_end_ms = None;
        state.local_speaker_signal_quality_sufficient = None;
        state.local_speaker_observation_end_ms = None;
        state.owner_isolation_frozen = false;
        state.owner_isolation_ceiling_text.clear();
        state.owner_isolation_ceiling_segments.clear();
        state.wake_speaker_phrase = Some(wake_phrase.to_string());
        state.wake_target_speech_end_ms = None;
        log::info!(
            "[asr] local session-speaker tracking anchored to wake speaker wake_owner_verified={wake_owner_verified}"
        );
    }

    pub fn local_speaker_tracking_active(&self) -> bool {
        self.state.lock().local_speaker_tracking_enabled
    }

    /// Defense against a repeated activation loss (the r10-r17 field shape:
    /// live accept ran, yet the final arbitration saw tracking=false). The
    /// session tracker calls this on every body observation; if the anchor
    /// notes vanished, re-apply them before more evidence accumulates.
    pub fn reanchor_local_speaker_tracking_if_lost(
        &self,
        wake_phrase: &str,
        wake_owner_verified: bool,
    ) -> bool {
        {
            let state = self.state.lock();
            if state.local_speaker_tracking_enabled && state.wake_speaker_phrase.is_some() {
                return false;
            }
        }
        self.note_local_speaker_tracking_started_with_owner(wake_phrase, wake_owner_verified);
        log::warn!(
            "[asr] local session-speaker tracking re-anchored after activation loss wake_owner_verified={wake_owner_verified}"
        );
        true
    }

    pub(crate) fn set_streaming_event_callback(&self, callback: Option<StreamingEventCallback>) {
        *self.streaming_event_callback.lock() = callback;
    }

    pub(crate) fn set_pipeline_observation(
        &self,
        observation: Arc<crate::observability::EmbeddedAudioPipelineObservation>,
    ) {
        *self.pipeline_observation.lock() = Some(observation);
    }

    fn pipeline_observation(
        &self,
    ) -> Option<Arc<crate::observability::EmbeddedAudioPipelineObservation>> {
        self.pipeline_observation.lock().clone()
    }

    fn take_queued_audio_observation(&self, sequence: i32) -> Option<Vec<AudioSourceShare>> {
        self.queued_audio_observations
            .lock()
            .remove(&(self.asr_stream_id(), sequence))
    }

    fn claim_queued_audio_observation(&self, sequence: i32) -> Option<Vec<AudioSourceShare>> {
        let stream_id = self.asr_stream_id();
        let shares = self
            .queued_audio_observations
            .lock()
            .remove(&(stream_id, sequence))?;
        self.in_flight_audio_observations
            .lock()
            .insert((stream_id, sequence), shares.clone());
        Some(shares)
    }

    fn claim_audio_source_for_worker(
        &self,
        stream_id: u64,
        sequence: i32,
        frame_bytes: usize,
    ) -> Vec<AudioSourceShare> {
        let shares = self
            .queued_audio_observations
            .lock()
            .remove(&(stream_id, sequence));
        match shares {
            Some(shares) => {
                self.in_flight_audio_observations
                    .lock()
                    .insert((stream_id, sequence), shares.clone());
                shares
            }
            None => {
                // A missing registry entry is a bookkeeping failure, not
                // permission to borrow whichever capture happens to be
                // current. Keep the PCM on the real transport path, but
                // leave it unassociated so it cannot alter another capture's
                // ledger.
                self.record_unassociated_audio(frame_bytes);
                vec![AudioSourceShare {
                    observation: None,
                    segment_id: None,
                    bytes: frame_bytes,
                    destination_range: None,
                    source_interval: None,
                }]
            }
        }
    }

    fn asr_stream_id(&self) -> u64 {
        self.asr_stream_id.load(Ordering::Relaxed)
    }

    fn release_in_flight_audio_observation(&self, stream_id: u64, sequence: i32) {
        self.in_flight_audio_observations
            .lock()
            .remove(&(stream_id, sequence));
    }

    fn abandon_queued_audio_observations(&self, stream_id: u64) -> usize {
        let queued = {
            let mut queued = self.queued_audio_observations.lock();
            let keys = queued
                .keys()
                .filter(|(key_stream_id, _)| *key_stream_id == stream_id)
                .copied()
                .collect::<Vec<_>>();
            keys.into_iter()
                .filter_map(|(stream_id, sequence)| {
                    queued
                        .remove(&(stream_id, sequence))
                        .map(|shares| (stream_id, sequence, shares))
                })
                .collect::<Vec<_>>()
        };
        let queued_count = queued.len();
        for (stream_id, sequence, shares) in queued {
            let frame_bytes = shares.iter().map(|share| share.bytes).sum();
            let destination_range = destination_range_for_source_shares(&shares, frame_bytes);
            record_asr_abandoned_for_source_shares(
                stream_id,
                sequence,
                destination_range,
                &shares,
                frame_bytes,
            );
        }
        queued_count
    }

    fn take_pending_audio(
        &self,
    ) -> (
        usize,
        Option<crate::observability::PcmRange>,
        Vec<AudioSourceShare>,
    ) {
        let mut state = self.state.lock();
        let bytes = state.pending_audio.len();
        let destination_range = if state.destination_ledger_incomplete {
            None
        } else {
            state.pending_audio_destination_start.and_then(|start| {
                crate::observability::PcmRange::from_start_and_bytes(start, bytes)
            })
        };
        let shares = drain_audio_source_runs(&mut state.pending_audio_sources, bytes);
        state.pending_audio.clear();
        state.pending_audio_destination_start = None;
        (bytes, destination_range, shares)
    }

    fn abandon_pending_audio(&self) {
        let (bytes, destination_range, shares) = { self.take_pending_audio() };
        if bytes > 0 {
            record_asr_unresolved_for_source_shares(
                self.asr_stream_id(),
                destination_range,
                &shares,
                bytes,
            );
        }
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

        let (ws, _resp) = tokio::time::timeout(
            WEBSOCKET_CONNECT_TIMEOUT,
            connect_ws_with_network_policy(request, &self.proxy_config),
        )
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
        // Serialize the stream-id transition with consume/queue publication.
        // Otherwise a same-object reopen can drain entries belonging to the
        // new stream, or publish old entries after the cleanup pass.
        {
            let _delivery_queue_guard = self.delivery_queue_lock.lock();
            let previous_stream_id = self.asr_stream_id();
            self.abandon_queued_audio_observations(previous_stream_id);
            self.asr_stream_id
                .store(next_volcengine_asr_stream_id(), Ordering::Relaxed);
            let mut st = self.state.lock();
            // Automatic wake configures the verified owner anchor immediately
            // before opening the cloud stream. Preserve that pending-session
            // configuration across the transport reset: clearing
            // `local_wake_owner_verified` here made the strict owner ledger keep
            // the body provisional while the visual-only ledger also refused to
            // render it, leaving the capsule blank until provider settlement.
            let pending_speaker_anchor = OwnerContinuitySnapshot::capture(&st);
            st.pending_audio.clear();
            st.pending_audio_sources.clear();
            st.next_input_destination_offset = 0;
            st.destination_ledger_incomplete = false;
            st.pending_audio_destination_start = None;
            st.next_sequence = 1;
            st.bytes_sent = 0;
            st.frames_sent = 0;
            st.response_frames_seen = 0;
            st.partial_updates_seen = 0;
            // Keep the producer closed while the new writer and worker are
            // installed. This prevents a same-object reopen from accepting
            // PCM into the old sender during that transition window.
            st.is_connected = false;
            st.final_tx = Some(tx);
            st.runtime = Some(Handle::current());
            st.start = Some(Instant::now());
            st.finishing = false;
            st.last_partial_text.clear();
            st.best_transcript_text.clear();
            st.best_transcript_committed_at = None;
            st.pause_early_recent_revisions.clear();
            st.best_transcript_segments.clear();
            st.best_untimed_window.clear();
            st.optimistic_preview_text.clear();
            st.optimistic_preview_segments.clear();
            st.optimistic_untimed_window.clear();
            st.last_emitted_preview_text.clear();
            st.last_emitted_visual_preview_text.clear();
            st.last_server_audio_duration_ms = None;
            st.stop_boundary_server_audio_ms = None;
            st.last_server_response_at = None;
            st.bytes_sent_since_response = 0;
            st.post_response_gap_logged = false;
            st.target_speaker_id = None;
            st.target_speech_end_ms = None;
            st.wake_target_speech_end_ms = None;
            st.stable_attributed_speech_end_ms = None;
            st.local_audio_duration_ms = None;
            st.local_speech_end_ms = None;
            st.qualified_owner_speech_end_ms = None;
            st.local_speaker_signal_quality_sufficient = None;
            st.local_speaker_observation_end_ms = None;
            st.local_target_speech_end_ms = None;
            st.local_non_target_speech_end_ms = None;
            st.local_sustained_non_target_speech_end_ms = None;
            st.local_speaker_classification = None;
            st.local_target_confirmed = false;
            st.local_consecutive_target = 0;
            st.local_consecutive_non_target = 0;
            st.local_consecutive_transcript_hard_non_target = 0;
            st.local_confirmed_transcript_non_target_end_ms = None;
            st.local_consecutive_transcript_foreign_hint = 0;
            st.local_confirmed_transcript_foreign_hint_end_ms = None;
            st.local_preview_foreign_hint_end_ms = None;
            st.local_owner_handoff_suspected = false;
            st.local_owner_handoff_recovery_targets = 0;
            st.local_consecutive_strong_non_target = 0;
            st.local_owner_absence_run_started_ms = None;
            st.local_owner_absence_run_confirmed = false;
            st.local_speaker_evidence.clear();
            st.local_unreliable_speaker_evidence_end_ms.clear();
            pending_speaker_anchor.restore(&mut st);
            st.owner_isolation_frozen = false;
            st.owner_isolation_ceiling_text.clear();
            st.owner_isolation_ceiling_segments.clear();
            st.distinct_speaker_target_final_text = None;
            st.wake_bound_single_speaker_final_text = None;
            st.transcript_evidence.reset();
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
        {
            let _delivery_queue_guard = self.delivery_queue_lock.lock();
            *self.audio_tx.lock() = Some(audio_tx);
            self.state.lock().is_connected = true;
        }
        let writer_for_worker = Arc::clone(&self.writer);
        let pending_for_worker = Arc::clone(&self.pending_sends);
        let notify_for_worker = Arc::clone(&self.send_done);
        let failed_delivery = Arc::downgrade(self);
        let worker_stream_id = self.asr_stream_id();
        let role_label = self.session_options.endpoint.label();
        tokio::spawn(async move {
            while let Some((seq, chunk)) = audio_rx.recv().await {
                let chunk_bytes = chunk.len();
                let Some(asr) = failed_delivery.upgrade() else {
                    break;
                };
                // Claim the source before the first await. Cancellation only
                // abandons entries that remain queued; the claimed frame owns
                // its eventual success/failure settlement in this worker.
                // A missing observation must never suppress the real PCM send.
                // Attribute it explicitly as unknown when an observation
                // ledger is available; the transport path remains unchanged.
                let source_shares =
                    asr.claim_audio_source_for_worker(worker_stream_id, seq, chunk_bytes);
                let destination_range =
                    destination_range_for_source_shares(&source_shares, chunk_bytes);
                record_asr_claimed_for_source_shares(
                    worker_stream_id,
                    seq,
                    destination_range,
                    &source_shares,
                );
                let frame = frame::build(
                    MessageType::AudioOnlyRequest,
                    Flags::PositiveSequence,
                    Serialization::None,
                    &chunk,
                    Some(seq),
                );
                let send_result =
                    if let Err(error) = send_binary(&writer_for_worker, frame.clone()).await {
                        // Live 2c5042c2: preroll flush after overlap-rearm timed out
                        // at 1200 ms, so the capsule stayed empty then no-body ended.
                        log::warn!(
                            "[asr] {} audio frame seq={} send retry after: {}",
                            role_label,
                            seq,
                            error
                        );
                        if let Err(error) = send_binary(&writer_for_worker, frame).await {
                            log::error!(
                                "[asr] {} audio frame seq={} send 失败: {}",
                                role_label,
                                seq,
                                error
                            );
                            Err(error)
                        } else {
                            Ok(())
                        }
                    } else {
                        Ok(())
                    };

                asr.release_in_flight_audio_observation(worker_stream_id, seq);
                match send_result {
                    Ok(()) => {
                        record_asr_send_completed_for_source_shares(
                            worker_stream_id,
                            seq,
                            destination_range,
                            &source_shares,
                            chunk_bytes,
                        );
                        if pending_for_worker.fetch_sub(1, Ordering::SeqCst) == 1 {
                            notify_for_worker.notify_waiters();
                        }
                    }
                    Err(error) => {
                        record_asr_send_failed_for_source_shares(
                            worker_stream_id,
                            seq,
                            destination_range,
                            &source_shares,
                            chunk_bytes,
                        );
                        let _delivery_queue_guard = asr.delivery_queue_lock.lock();
                        asr.mark_audio_delivery_failed(error);
                        let abandoned_queued_frames =
                            asr.abandon_queued_audio_observations(worker_stream_id);
                        *asr.audio_tx.lock() = None;
                        // The failed frame and every frame still represented in
                        // the sequence registry are terminally abandoned. Keep
                        // pending_sends tied to those actual queue entries;
                        // never clear it as a diagnostic shortcut.
                        let terminal_frames = abandoned_queued_frames.saturating_add(1);
                        let _ = pending_for_worker.fetch_update(
                            Ordering::SeqCst,
                            Ordering::SeqCst,
                            |pending| Some(pending.saturating_sub(terminal_frames)),
                        );
                        notify_for_worker.notify_waiters();
                        break;
                    }
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
            loop {
                let msg = read.next().await;
                let Some(this) = weak_self.upgrade() else {
                    break;
                };
                match msg {
                    Some(Ok(Message::Binary(data))) => {
                        if !this.handle_frame(&data) {
                            break;
                        }
                    }
                    Some(Ok(Message::Close(frame))) => {
                        if !this.state.lock().is_connected {
                            break;
                        }
                        log::warn!(
                            "[asr] receive loop received close frame without final code={}",
                            frame.as_ref().map(|frame| u16::from(frame.code)).unwrap_or(0)
                        );
                        // 服务端没发 final 就关连接 → 用最近一次 partial 兜底，不丢已识别的文字。
                        this.fallback_to_partial_or_error(VolcengineASRError::NoFinalResult);
                        break;
                    }
                    Some(Ok(_)) => { /* ignore text/ping/pong */ }
                    Some(Err(e)) => {
                        if !this.state.lock().is_connected {
                            break;
                        }
                        log::error!("[asr] receive loop error: {}", e);
                        // 网络中断同样回退到 partial，让用户至少拿到已经识别的部分。
                        this.fallback_to_partial_or_error(VolcengineASRError::ConnectionFailed(
                            e.to_string(),
                        ));
                        break;
                    }
                    None => {
                        if this.state.lock().is_connected {
                            log::warn!("[asr] receive loop ended without final frame");
                            this.fallback_to_partial_or_error(
                                VolcengineASRError::ConnectionFailed(
                                    "provider receive stream ended without final frame".into(),
                                ),
                            );
                        }
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
        let result = self.final_frame_result
            .get_or_init(|| self.send_last_frame_once())
            .await
            .clone();
        // A stalled socket has already sealed its owner ledger and resolved
        // final_rx. Sending a negative frame to that closed socket necessarily
        // fails. If the ledger covers the last locally confirmed owner speech,
        // let the coordinator consume that result instead of replaying the
        // entire recording and allowing a second diarization pass to erase a
        // continuation that was already recognized and delivered live.
        if result.is_err() && self.stalled_ledger_covers_owner_speech() {
            log::info!("[asr] stalled stream final uses owner-covered session ledger; replay skipped");
            return Ok(());
        }
        result
    }

    fn stalled_ledger_covers_owner_speech(&self) -> bool {
        let st = self.state.lock();
        if !matches!(
            &st.audio_delivery_readiness,
            AudioDeliveryReadiness::Failed(VolcengineASRError::ConnectionFailed(reason))
                if reason.starts_with("uplink stall:")
        ) || st.is_connected
            || st.final_tx.is_some()
            || st.owner_isolation_frozen
            || st.best_transcript_text.trim().is_empty()
            || st.best_transcript_committed_at.is_none_or(|at| at.elapsed() < Duration::from_secs(1))
        {
            return false;
        }
        let Some(transcript_end_ms) = st.best_transcript_segments
            .iter()
            .filter_map(|segment| segment.end_ms)
            .max()
            .and_then(|end_ms| u64::try_from(end_ms).ok())
        else {
            return false;
        };
        let Some(owner_end_ms) = st.qualified_owner_speech_end_ms
            .into_iter()
            .chain(st.local_target_speech_end_ms)
            .max()
        else {
            return false;
        };
        let foreign_after_owner = st.local_non_target_speech_end_ms
            .into_iter()
            .chain(st.local_sustained_non_target_speech_end_ms)
            .any(|end_ms| end_ms > owner_end_ms);
        !foreign_after_owner
            && owner_end_ms <= transcript_end_ms.saturating_add(500)
            && st.last_server_audio_duration_ms.is_some_and(|server_ms| server_ms >= owner_end_ms)
    }

    #[cfg(test)]
    pub(crate) fn install_test_final_wait_barrier(&self) -> (Arc<Notify>, Arc<Notify>) {
        self.install_test_final_wait_barrier_with_provider_result(Ok(RawTranscript {
            text: String::new(),
            duration_ms: 0,
        }))
    }

    #[cfg(test)]
    pub(crate) fn install_test_final_wait_barrier_with_result(
        &self,
        final_result: RawTranscript,
    ) -> (Arc<Notify>, Arc<Notify>) {
        self.install_test_final_wait_barrier_with_provider_result(Ok(final_result))
    }

    #[cfg(test)]
    pub(crate) fn install_test_final_wait_barrier_with_provider_result(
        &self,
        final_result: Result<RawTranscript, VolcengineASRError>,
    ) -> (Arc<Notify>, Arc<Notify>) {
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        {
            let mut state = self.state.lock();
            let (final_tx, final_rx) = oneshot::channel();
            state.final_tx = Some(final_tx);
            *self.final_rx.lock() = Some(final_rx);
        }
        *self.test_final_wait_barrier.lock() = Some(Arc::new(TestFinalWaitBarrier {
            entered: Arc::clone(&entered),
            entered_flag: AtomicBool::new(false),
            release: Arc::clone(&release),
            final_result,
        }));
        (entered, release)
    }

    #[cfg(test)]
    async fn await_test_final_wait_barrier(&self) {
        let barrier = self.test_final_wait_barrier.lock().clone();
        let Some(barrier) = barrier else {
            return;
        };
        let released = barrier.release.notified();
        barrier.entered_flag.store(true, Ordering::SeqCst);
        barrier.entered.notify_waiters();
        released.await;

        if let Some(final_tx) = self.state.lock().final_tx.take() {
            let _ = final_tx.send(barrier.final_result.clone());
        }
    }

    async fn send_last_frame_once(&self) -> Result<(), VolcengineASRError> {
        // Seal immediately so a proactive endpoint finalization cannot race
        // later firmware-drain PCM into the stream after its negative frame.
        {
            let mut state = self.state.lock();
            state.finishing = true;
            state.stop_boundary_server_audio_ms = state.last_server_audio_duration_ms;
        }
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
        let (leftover, destination_range, source_shares) = {
            let mut st = self.state.lock();
            if st.pending_audio.is_empty() {
                (None, None, Vec::new())
            } else {
                let leftover = std::mem::take(&mut st.pending_audio);
                let destination_range = if st.destination_ledger_incomplete {
                    None
                } else {
                    st.pending_audio_destination_start.and_then(|start| {
                        crate::observability::PcmRange::from_start_and_bytes(start, leftover.len())
                    })
                };
                let source_shares =
                    drain_audio_source_runs(&mut st.pending_audio_sources, leftover.len());
                st.pending_audio_destination_start = None;
                (Some(leftover), destination_range, source_shares)
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
                st.bytes_sent_since_response += len;
                st.frames_sent += 1;
            }
            record_asr_queued_for_source_shares(
                self.asr_stream_id(),
                seq,
                destination_range,
                &source_shares,
                len,
            );
            match self.send_finish_frame(frame, finish_send_deadline).await {
                Ok(()) => {
                    record_asr_send_completed_for_source_shares(
                        self.asr_stream_id(),
                        seq,
                        destination_range,
                        &source_shares,
                        len,
                    );
                }
                Err(error) => {
                    record_asr_send_failed_for_source_shares(
                        self.asr_stream_id(),
                        seq,
                        destination_range,
                        &source_shares,
                        len,
                    );
                    return Err(error);
                }
            }
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
        #[cfg(test)]
        self.await_test_final_wait_barrier().await;
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
        #[cfg(test)]
        if self.test_final_wait_barrier.lock().is_some() {
            let _ = frame;
            return Ok(());
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
            Err(_) => Err(self.provider_final_wait_timeout(timeout)),
        }
    }

    /// 2026-09-22 arbitration floor predicate (see the call-site comment for
    /// the live case): a wake-accepted tracked session whose provider stream
    /// transcribed a substantial body must not end with only the local
    /// filter's wake remnant. Env kill-switch LISTENER_DISABLE_ARBITRATION_FLOOR=1
    /// restores the strict filter path.
    fn arbitration_floor_should_override(
        authority: &crate::speech_decision_kernel::FinalTranscriptAuthority,
        tracking_enabled: bool,
        provider_chars: usize,
        filtered_chars: usize,
        ledger_chars: usize,
        optimistic_chars: usize,
    ) -> bool {
        if std::env::var("LISTENER_DISABLE_ARBITRATION_FLOOR").as_deref() == Ok("1") {
            return false;
        }
        tracking_enabled
            && matches!(
                authority,
                crate::speech_decision_kernel::FinalTranscriptAuthority::SpeakerFiltered
            )
            && provider_chars >= 20
            && filtered_chars <= 6
            // Last resort only: when the session ledger or the optimistic
            // preview kept substantial owner-confirmed text, those recovery
            // paths are better than the raw provider stream (see
            // protocol_final_preserves_continuously_confirmed_owner_preview).
            && ledger_chars <= 6
            && optimistic_chars <= 6
    }

    fn provider_final_wait_timeout(&self, waited: Duration) -> VolcengineASRError {
        if let Some((sent_audio_ms, transcript_end_ms)) = self.final_partial_coverage_gap() {
            log::error!(
                "[asr] final transcript coverage incomplete after full provider timeout: sent_audio_ms={sent_audio_ms} transcript_end_ms={transcript_end_ms}"
            );
            self.cancel();
            return VolcengineASRError::FinalResultCoverageIncomplete {
                sent_audio_ms,
                transcript_end_ms,
            };
        }
        log::error!(
            "[asr] provider final result timed out after {} ms",
            waited.as_millis()
        );
        self.cancel();
        VolcengineASRError::FinalResultTimeout
    }

    /// Session-ledger snapshot eligible to stand in for the provider final:
    /// the stream is finishing, the ledger is non-empty, attribution never
    /// froze an isolation ceiling and never saw a non-target window, and the
    /// text has been byte-stable for at least `min_stable`.
    fn stable_ledger_final_snapshot(&self, min_stable: Duration) -> Option<RawTranscript> {
        let st = self.state.lock();
        if !st.finishing || st.owner_isolation_frozen || st.local_non_target_speech_end_ms.is_some()
        {
            return None;
        }
        let committed_at = st.best_transcript_committed_at?;
        if st.best_transcript_text.trim().is_empty() {
            return None;
        }
        if committed_at.elapsed() < min_stable {
            return None;
        }
        let duration_ms = st
            .best_transcript_segments
            .iter()
            .filter_map(|segment| segment.end_ms)
            .max()
            .and_then(|end_ms| u64::try_from(end_ms).ok())
            .unwrap_or_else(|| (st.bytes_sent as f64 / BYTES_PER_MS) as u64);
        Some(RawTranscript {
            text: st.best_transcript_text.clone(),
            duration_ms,
        })
    }

    /// Pause-early-delivery ledger snapshot (2026-09-22 跟手①).
    ///
    /// Cleanliness contract: no owner-isolation ceiling is frozen (hard
    /// multi-speaker evidence), and an owner-ledger prefix has remained
    /// unchanged across all revisions for `min_stable`. Later words can keep
    /// growing without restarting that prefix's timer. Unlike the early-seal
    /// snapshot this deliberately does
    /// NOT require `finishing` and does NOT hard-fail on
    /// `local_non_target_speech_end_ms`: in a quiet room the owner's own
    /// voice regularly classifies into the sub-owner band (K 行实测 0.30 带),
    /// so that bit fires on the user themself and starves the whole
    /// mechanism (14:32 实锤：5.6 秒安静窗一次没触发). The ledger text is
    /// already attribution-filtered upstream, and the final arbitration at
    /// STOP keeps full authority to revise — the coordinator's
    /// remainder/mismatch logic handles that revision.
    pub fn pause_early_delivery_ledger_snapshot(
        &self,
        min_stable: Duration,
    ) -> Option<RawTranscript> {
        let mut st = self.state.lock();
        if st.owner_isolation_frozen {
            return None;
        }
        let committed_at = st.best_transcript_committed_at?;
        if st.best_transcript_text.trim().is_empty() {
            return None;
        }
        let current = st.best_transcript_text.clone();
        let stable_prefix = rolling_stable_transcript_prefix(
            &mut st.pause_early_recent_revisions,
            &current,
            committed_at,
            Instant::now(),
            min_stable,
        )?;
        if stable_prefix.len() < current.len()
            && st.local_speaker_tracking_enabled
            && (st.local_owner_handoff_suspected
                || !matches!(
                    local_owner_continuity(&st),
                    LocalOwnerContinuity::Confirmed | LocalOwnerContinuity::Compatible
                ))
        {
            // The rolling path is earlier than the old whole-ledger pause
            // path. It needs current owner evidence, not merely an old owner
            // prefix that survived a possible speaker handoff.
            return None;
        }
        let duration_ms = st
            .best_transcript_segments
            .iter()
            .filter_map(|segment| segment.end_ms)
            .max()
            .and_then(|end_ms| u64::try_from(end_ms).ok())
            .unwrap_or_else(|| (st.bytes_sent as f64 / BYTES_PER_MS) as u64);
        Some(RawTranscript {
            text: stable_prefix,
            duration_ms,
        })
    }

    /// Persistent pause-early blocker reason for gate diagnostics. `None`
    /// means no persistent blocker (empty/immature ledgers resolve on their
    /// own as the session progresses and are not worth a log line).
    pub fn pause_early_delivery_persistent_block_reason(&self) -> Option<&'static str> {
        let st = self.state.lock();
        if st.owner_isolation_frozen {
            return Some("owner_isolation_frozen");
        }
        None
    }

    /// 2026-09-23 tkg 后诊断:停顿落屏全静默(用户三题验收全不通过,19s 会话
    /// 零次触发)但现有代码只在"持久阻断"时记日志,未成熟账本的静默 None 无迹
    /// 可查。给端点看门狗第一拍一锤定音的全量门状态。一次性,不刷屏。
    pub fn pause_early_delivery_gate_diag(&self) -> (bool, Option<u128>, usize) {
        let st = self.state.lock();
        (
            st.owner_isolation_frozen,
            st.best_transcript_committed_at
                .map(|at| at.elapsed().as_millis()),
            st.best_transcript_text.chars().count(),
        )
    }

    /// Cleanliness read for the final-side pause-early mismatch recovery:
    /// true when the session never froze an isolation ceiling and never saw
    /// local non-target speech — a final rewrite in such a clean session is
    /// a cloud revision, and the rewritten tail is safe to append. Under
    /// interference the rewrite may re-attribute foreign speech, so the
    /// delivered owner-verified prefix stays and the tail is dropped.
    pub fn pause_early_final_clean_session(&self) -> bool {
        let st = self.state.lock();
        !st.owner_isolation_frozen && st.local_non_target_speech_end_ms.is_none()
    }

    /// await_final_result + stability early-seal (2026-09-21 跟手优化).
    ///
    /// In clean sessions the endpoint's own inactivity budget (1s inactive +
    /// the continuation window) proves everything after the last ledger change
    /// is silence, so the stable ledger is already a complete final. Give the
    /// provider final `EARLY_SEAL_FINAL_HEAD_START` to win the race (identical
    /// behaviour when the final is prompt), then seal from the ledger and
    /// cancel the stream instead of waiting out the two-pass round trip.
    /// When the ledger is not eligible the call degrades to the normal wait.
    pub async fn await_final_result_with_early_seal(
        &self,
    ) -> Result<RawTranscript, VolcengineASRError> {
        let rx = self.final_rx.lock().take();
        let Some(mut rx) = rx else {
            return Err(VolcengineASRError::NoFinalResult);
        };
        tokio::select! {
            biased;
            result = &mut rx => match result {
                Ok(result) => result,
                Err(_) => Err(VolcengineASRError::NoFinalResult),
            },
            _ = tokio::time::sleep(EARLY_SEAL_FINAL_HEAD_START) => {
                if let Some(snapshot) = self.stable_ledger_final_snapshot(EARLY_SEAL_MIN_LEDGER_STABLE) {
                    log::info!(
                        "[asr] early-sealed final from stable session ledger chars={} stable_budget_ms={}",
                        snapshot.text.chars().count(),
                        EARLY_SEAL_MIN_LEDGER_STABLE.as_millis()
                    );
                    self.cancel();
                    return Ok(snapshot);
                }
                // Ledger not stable enough (fresh revision, interference
                // evidence, or empty) — the real final is the only authority.
                let remaining = FINAL_RESULT_TIMEOUT.saturating_sub(EARLY_SEAL_FINAL_HEAD_START);
                match tokio::time::timeout(remaining, &mut rx).await {
                    Ok(Ok(result)) => result,
                    Ok(Err(_)) => Err(VolcengineASRError::NoFinalResult),
                    Err(_) => Err(self.provider_final_wait_timeout(remaining)),
                }
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
        #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
        if let Some((stream, task)) = self.target_speaker_final_task.lock().take() {
            stream.cancel();
            task.abort();
        }
        let (runtime, pending_audio) = {
            // Do not remove queued source entries here. The worker owns the
            // actual queue lifecycle and will settle each received frame; a
            // frame that is already in-flight is never reclassified as a
            // cancel-side abandonment.
            let _delivery_queue_guard = self.delivery_queue_lock.lock();
            self.mark_audio_delivery_cancelled();
            // Close the producer-side state boundary before taking the
            // leftover PCM. A concurrent consume either completes its frame
            // registration under this guard or observes the terminal state;
            // it cannot add a half-frame after this snapshot is taken.
            let pending_audio = self.take_pending_audio();
            let mut st = self.state.lock();
            st.is_connected = false;
            let runtime = st.runtime.clone();
            drop(st);
            *self.audio_tx.lock() = None;
            (runtime, pending_audio)
        };
        let (pending_bytes, pending_destination_range, pending_shares) = pending_audio;
        if pending_bytes > 0 {
            record_asr_unresolved_for_source_shares(
                self.asr_stream_id(),
                pending_destination_range,
                &pending_shares,
                pending_bytes,
            );
        }
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

    /// Detect an initial uplink blackhole before the provider's own timeout.
    /// Once any response has arrived, later result silence is ambiguous: the
    /// provider can keep receiving audio without emitting new transcript rows.
    /// Aborting that healthy socket loses all later speech until STOP replay.
    pub fn abort_if_uplink_stalled(&self) -> bool {
        const UPLINK_STALL_SILENCE: Duration = Duration::from_millis(4_500);
        const UPLINK_STALL_MIN_SENT_BYTES: usize = 48_000; // ~1.5s @ 16kHz/16bit
        let (connected, response_frames_seen, stalled, sent_bytes, silence_ms, post_response_gap) = {
            let mut st = self.state.lock();
            // The session start remains the clock when no response exists.
            // Keep the last-response clock for diagnostics; the gate below
            // excludes streams that have already produced a response.
            let (stalled, silence_ms) = match st.last_server_response_at {
                Some(at) => {
                    let elapsed = at.elapsed();
                    (elapsed >= UPLINK_STALL_SILENCE, elapsed.as_millis())
                }
                None => match st.start {
                    Some(start) => {
                        let elapsed = start.elapsed();
                        (elapsed >= UPLINK_STALL_SILENCE, elapsed.as_millis())
                    }
                    // 没有时钟基准（测试构造/未开流）不判死。
                    None => (false, 0),
                },
            };
            let post_response_gap = st.is_connected
                && st.response_frames_seen > 0
                && stalled
                && st.bytes_sent_since_response >= UPLINK_STALL_MIN_SENT_BYTES
                && !st.post_response_gap_logged;
            if post_response_gap {
                st.post_response_gap_logged = true;
            }
            (
                st.is_connected,
                st.response_frames_seen,
                stalled,
                st.bytes_sent_since_response,
                silence_ms,
                post_response_gap,
            )
        };
        if post_response_gap {
            log::info!(
                "[asr] post-response result gap observed silence_ms={silence_ms} sent_bytes={sent_bytes}; keeping socket open"
            );
        }
        if !connected
            || response_frames_seen > 0
            || !stalled
            || sent_bytes < UPLINK_STALL_MIN_SENT_BYTES
        {
            return false;
        }
        log::warn!(
            "[asr] uplink stall detected: no initial server response silence_ms={silence_ms} sent_bytes={sent_bytes} — aborting stream for fast recovery (cloud 45000081 would fire at 8s)"
        );
        // 网络定位数据（2026-09-22 用户拍板）：黑洞掐线时探测火山端点+对照站，
        // 判读行进 decisions.log，积累几天即可回答"哪一跳在吞"。
        crate::net_health::log_network_health_probe_async("uplink_stall_abort");
        // 与 cancel() 同款异步关写端：读循环见 EOF 退出，发送 worker 在下一次
        // 发送失败/通道关闭时自然终止。
        let runtime = self.state.lock().runtime.clone();
        if let Some(runtime) = runtime {
            let writer = Arc::clone(&self.writer);
            runtime.spawn(async move {
                if let Some(mut w) = writer.lock().await.take() {
                    let _ = w.close().await;
                }
            });
        }
        // 与 45000081 错误帧同一出口：账本兜底或报错，均许可保留音频重放。
        self.fallback_to_partial_or_error(VolcengineASRError::ConnectionFailed(
            "uplink stall: audio kept flowing but server went silent".into(),
        ));
        true
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
            "accelerate_score": self.session_options.accelerate_score.min(20),
            "enable_itn": true,
            "enable_punc": true,
            "show_utterances": true,
            "enable_speaker_info": true,
            "ssd_version": "200",
            "result_type": self.session_options.result_type.as_str(),
        });
        if let Some(end_window_size_ms) = self.session_options.end_window_size_ms {
            request["end_window_size"] = Value::from(end_window_size_ms);
        }
        #[cfg(test)]
        if let Ok(version) = std::env::var("LISTENER_PROVIDER_CADENCE_SSD_VERSION") {
            request["ssd_version"] = Value::String(version);
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
            // 断流类错误帧（45000081 等）顺带探测网络分层，定位数据。
            crate::net_health::log_network_health_probe_async("provider_error_frame");
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
            // 任何服务端应答（含静音期的同文重发帧）都证明上行链路活着；
            // 上行黑洞探测的时钟以此为基准重新起算。
            st.last_server_response_at = Some(Instant::now());
            st.bytes_sent_since_response = 0;
            st.post_response_gap_logged = false;
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

        #[cfg(test)]
        self.state
            .lock()
            .diagnostic_provider_responses
            .push(json.clone());

        let has_final = parsed.is_final();
        let trace_frame = self.state.lock().response_frames_seen;
        self.record_diagnostic_trace(
            trace_frame,
            has_final,
            "provider_raw_result",
            result
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        );
        self.record_diagnostic_trace(
            trace_frame,
            has_final,
            "normalized_result",
            result
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        );
        {
            let provider_text = result
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let diarization_present = result
                .get("utterances")
                .and_then(Value::as_array)
                .is_some_and(|utterances| {
                    utterances
                        .iter()
                        .any(|utterance| utterance_speaker_id(utterance).is_some())
                });
            let mut state = self.state.lock();
            let coverage_ms = state.last_server_audio_duration_ms;
            state.transcript_evidence.note_provider_revision(
                provider_text,
                coverage_ms,
                diarization_present,
                has_final,
            );
        }
        let (speaker_filtered_result, target_speaker_update, provisional_holds_endpoint) = {
            let mut state = self.state.lock();
            let local_speaker_tracking_enabled = state.local_speaker_tracking_enabled;
            let local_speaker_evidence = state.local_speaker_evidence.clone();
            let wake_speaker_phrase = state.wake_speaker_phrase.clone();
            let prior_wake_speaker_end_ms = state.wake_target_speech_end_ms;
            let wake_owner_verified = state.local_wake_owner_verified;
            let local_speaker_profile_adaptive = state.local_speaker_profile_adaptive;
            let confirmed_foreign_hint_end_ms =
                state.local_confirmed_transcript_foreign_hint_end_ms;
            // Both endpoint-grade NonTarget and the independent, confirmed
            // transcript-grade mismatch describe the same physical exclusion
            // boundary. The latter remains non-destructive by itself, but a
            // stable provider foreign-speaker row overlapping that boundary
            // must not be selected into the owner-filtered result.
            let confirmed_non_target_speech_end_ms = state
                .local_non_target_speech_end_ms
                .into_iter()
                .chain(state.local_confirmed_transcript_non_target_end_ms)
                .max();
            let mut filtered = filter_result_to_target_speaker_with_local_evidence_and_anchor(
                result,
                &mut state.target_speaker_id,
                local_speaker_tracking_enabled,
                &local_speaker_evidence,
                wake_speaker_phrase.as_deref(),
                prior_wake_speaker_end_ms,
                wake_owner_verified,
                local_speaker_profile_adaptive,
                confirmed_non_target_speech_end_ms,
                confirmed_foreign_hint_end_ms,
            );
            recover_locally_supported_owner_prefix(&state, result, &mut filtered);
            #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
            if has_final {
                clamp_degraded_same_cluster_foreign_tail(&state, result, &mut filtered);
            }
            if let Some(wake_end_ms) = filtered.wake_target_speech_end_ms {
                state.wake_target_speech_end_ms = Some(wake_end_ms);
            }
            if filtered.stable_non_target_utterance_present {
                freeze_owner_isolation_at_filtered_result(&mut state, &filtered.result);
            }
            if has_final {
                state.wake_bound_single_speaker_final_text = None;
                let target_text = filtered
                    .result
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim();
                let single_owner_track_wake_bound = state.local_speaker_tracking_enabled
                    && state.local_wake_owner_verified
                    && filtered.speaker_info_present
                    && !filtered.stable_non_target_utterance_present
                    && !filtered.stable_other_speaker_present
                    && !filtered.response_local_body_alias_present;
                if single_owner_track_wake_bound && !target_text.is_empty() {
                    state.wake_bound_single_speaker_final_text = Some(target_text.to_string());
                    log::info!(
                        "[target-speaker] wake-bound single provider owner track retained chars={}",
                        target_text.chars().count()
                    );
                }
            }
            if has_final
                && (filtered.stable_other_speaker_present
                    || filtered.response_local_body_alias_present)
            {
                let target_text = filtered
                    .result
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim();
                if !target_text.is_empty() {
                    state.distinct_speaker_target_final_text = Some(target_text.to_string());
                    if filtered.response_local_body_alias_present {
                        log::info!(
                            "[target-speaker] verified contiguous body alias retained as provider owner track chars={}",
                            target_text.chars().count()
                        );
                    }
                }
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
            let owner_handoff_suspected =
                observe_provider_owner_handoff_candidate(&mut state, result);
            let provisional_holds_endpoint = pending_activity_advanced
                && !owner_handoff_suspected
                && local_speaker_allows_optimistic_preview(&state)
                && refresh_local_target_from_owner_preview_activity(&mut state);
            let update = target_speaker_update_from_state(
                &state,
                target_activity_advanced || provisional_holds_endpoint,
                pending_activity_advanced,
                false,
            );
            (filtered, update, provisional_holds_endpoint)
        };
        self.record_diagnostic_trace(
            trace_frame,
            has_final,
            "speaker_filtered_result",
            speaker_filtered_result
                .result
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        );
        if provisional_holds_endpoint {
            log::info!(
                "[asr] provisional body growth refreshed local target endpoint clock local_target_end_ms={:?}",
                target_speaker_update.local_target_speech_end_ms
            );
        }
        if target_speaker_update.speaker_id.is_some() {
            log::info!(
                "[asr] target-speaker state speaker_id={:?} stable_end_ms={:?} audio_duration_ms={:?} provider_audio_duration_ms={:?} local_speech_end_ms={:?} qualified_owner_end_ms={:?} qualified_advanced={} local_classification={:?} local_quality={:?} local_observation_end_ms={:?} local_target_end_ms={:?} local_non_target_end_ms={:?} local_tracking={} stable_attributed_end_ms={:?} pending_provisional={} target_advanced={} pending_advanced={}",
                target_speaker_update.speaker_id,
                target_speaker_update.target_speech_end_ms,
                target_speaker_update.audio_duration_ms,
                target_speaker_update.provider_audio_duration_ms,
                target_speaker_update.local_speech_end_ms,
                target_speaker_update.qualified_owner_speech_end_ms,
                target_speaker_update.qualified_owner_activity_advanced,
                target_speaker_update.local_speaker_classification_kind,
                target_speaker_update.local_speaker_signal_quality_sufficient,
                target_speaker_update.local_speaker_observation_end_ms,
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
        let provider_tail_excluded_from_preview = {
            let mut state = self.state.lock();
            update_local_preview_exclusion(&mut state, result)
                || state.local_owner_handoff_suspected
                || (!has_final && provider_has_pending_speaker_change(&state, result))
        };
        if provider_tail_excluded_from_preview && !has_final {
            log::info!("[asr] later provider row excluded from raw preview by sustained local owner absence; preserving visible text");
        }
        let owner_safe_provider_split_preview = !has_final
            && !provider_tail_excluded_from_preview
            && !speaker_filtered_result.stable_non_target_utterance_present
            && {
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
            && !self.state.lock().local_owner_handoff_suspected
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
            let raw_provider_preview = result
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let settled_preview = {
                let mut state = self.state.lock();
                // Dictation live preview follows the growing provider transcript.
                // Sentence-level speaker rows must not retract it (69→19 swallow).
                // Isolation is WeSep waveform extraction + owner clock.
                let filtered = if filtered_target_preview.trim().is_empty() {
                    state.best_transcript_text.clone()
                } else {
                    filtered_target_preview
                };
                let visible_len = spoken_content_len(&state.last_emitted_preview_text);
                // High priority: never retract shown owner text.
                // Low priority G: if a stable other-speaker row is present,
                // refuse to ADD that longer raw tail. Do not shrink.
                let new_tail_is_foreign = provider_tail_excluded_from_preview
                    || speaker_filtered_result.stable_non_target_utterance_present
                    || (speaker_filtered_result.stable_other_speaker_present
                        && local_owner_continuity(&state) == LocalOwnerContinuity::Other);
                let best = if new_tail_is_foreign
                    && spoken_content_len(&raw_provider_preview) > spoken_content_len(&filtered)
                {
                    if spoken_content_len(&filtered) >= visible_len && !filtered.trim().is_empty() {
                        filtered
                    } else {
                        String::new()
                    }
                } else if spoken_content_len(&raw_provider_preview) >= spoken_content_len(&filtered)
                    && !raw_provider_preview.trim().is_empty()
                {
                    raw_provider_preview
                } else {
                    filtered
                };
                let best_len = spoken_content_len(&best);
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
                self.record_diagnostic_trace(
                    trace_frame,
                    has_final,
                    "optimistic_partial_callback",
                    &preview,
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
        // Live 2026-09-11 02:07: owner_safe split set pending_unattributed,
        // which blocked settled preview, while this gate also blocked visual.
        // Capsule stayed empty ~5.4 s. Owner-safe split is growing owner text;
        // it must still display.
        if !has_final
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
                self.record_diagnostic_trace(
                    trace_frame,
                    has_final,
                    "visual_partial_callback",
                    &preview,
                );
                self.emit_visual_partial_transcript(&preview);
            }
        }
        let excluded_tail_provider_coverage = {
            let state = self.state.lock();
            // Use the existing owner-continuation exceptions as well as the
            // foreign veto. A cloud speaker-id split alone must not authorize
            // discarding a locally supported owner branch.
            final_explicit_non_owner_tail(&state, &speaker_filtered_result, result)
                .then(|| transcript_candidate_from_result(result))
        };
        if !has_final
            && pending_unattributed_speech
            && !provider_tail_excluded_from_preview
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
            let (optimistic_preview, inflated_merge_inputs) = {
                let mut state = self.state.lock();
                let merge_inputs = self.diagnostic_trace_enabled.then(|| {
                    (
                        state.optimistic_preview_text.clone(),
                        state.optimistic_untimed_window.clone(),
                        optimistic_candidate.text.clone(),
                    )
                });
                let (mut merged, segments, untimed_window) =
                    merge_optimistic_cumulative_view(
                        &state.optimistic_preview_text,
                        &state.optimistic_preview_segments,
                        &state.optimistic_untimed_window,
                        optimistic_candidate,
                        excluded_tail_provider_coverage.as_ref(),
                    );
                merged = trim_repeated_short_streaming_tail(&merged);
                // A rolling owner window may legitimately exceed the current
                // provider packet. Preserve the exact inputs only when that
                // happens, so a duplicated live tail can be reproduced from
                // the private per-session trace instead of guessing which
                // window or speaker-filtered candidate the merger received.
                let provider_text = result
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let inflated_merge_inputs = (spoken_content_len(provider_text) >= 16
                    && spoken_content_len(&merged) > spoken_content_len(provider_text))
                    .then_some(merge_inputs)
                    .flatten();
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
                    // preview-only until that identity exists and is therefore
                    // ineligible for empty-final/session-close recovery.
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
                let emitted = if should_emit {
                    state.last_emitted_preview_text = merged.clone();
                    state.partial_updates_seen += 1;
                    Some((merged, preview_holds_endpoint))
                } else {
                    None
                };
                (emitted, inflated_merge_inputs)
            };
            if let Some((previous, window, candidate)) = inflated_merge_inputs {
                self.record_diagnostic_trace(
                    trace_frame,
                    false,
                    "optimistic_merge_previous",
                    &previous,
                );
                self.record_diagnostic_trace(trace_frame, false, "optimistic_merge_window", &window);
                self.record_diagnostic_trace(
                    trace_frame,
                    false,
                    "optimistic_merge_candidate",
                    &candidate,
                );
            }
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
                        target_speaker_update_from_state(&state, true, false, false)
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
                self.record_diagnostic_trace(
                    trace_frame,
                    has_final,
                    "fallback_partial_callback",
                    &preview,
                );
                self.emit_partial_transcript(&preview);
                self.emit_streaming_event(VolcengineStreamingEvent::Partial(preview));
            }
        }
        // Protocol-final text selection is one atomic product decision. The
        // provider receive loop used to evaluate several overlapping boolean
        // recovery branches under separate state locks. A late local speaker
        // callback could therefore produce an internally inconsistent choice,
        // and any one fail-open branch could bypass another branch's foreign-
        // speaker veto. Build all facts from one owner-continuity snapshot and
        // ask the pure decision kernel for exactly one authority.
        let two_pass_empty_final = has_final && result_marks_two_pass_empty(result);
        let (
            final_authority,
            explicit_non_owner_tail,
            provider_raw_recovery_safe,
            provider_owner_recovery_safe,
            session_ledger_recovery_safe,
            optimistic_owner_recovery_safe,
            session_ledger_candidate,
        ) = {
            let state = self.state.lock();
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
            let explicit_non_owner_tail =
                final_explicit_non_owner_tail(&state, &speaker_filtered_result, result);
            let provider_raw_recovery_safe = has_final
                && final_unfiltered_provider_recovery_allowed(
                    &state,
                    &speaker_filtered_result,
                    result,
                );
            let open_session_body_recovery =
                open_session_wake_owned_body_recovery(&state, &speaker_filtered_result, result);
            if has_final && open_session_body_recovery {
                log::warn!(
                    "[asr] open-session wake-owned body recovery: unverified wake cannot veto the wake-anchored body provider_chars={} filtered_chars={}",
                    result
                        .get("text")
                        .and_then(Value::as_str)
                        .map_or(0, spoken_content_len),
                    spoken_content_len(target_text),
                );
            }
            let provider_owner_recovery_safe = has_final
                && (open_session_body_recovery
                    || (!speaker_filtered_result.stable_non_target_utterance_present
                        && (final_wake_only_provider_gap_is_owner_safe(&state, result, target_text)
                            || (!speaker_filtered_result.stable_unresolved_speaker_present
                                && (sequential_speaker_split_gap_is_owner_safe(
                                &state, result, target_text,
                            )
                            || final_unsegmented_provider_tail_is_owner_safe(
                                &state, result, target_text,
                            ))))));
            let optimistic_candidate_is_already_accepted = {
                let normalize = |text: &str| {
                    text.chars()
                        .filter(|ch| ch.is_alphanumeric())
                        .collect::<String>()
                };
                let candidate_key = normalize(optimistic_text);
                let accepted_key = normalize(&state.best_transcript_text);
                !candidate_key.is_empty()
                    && !accepted_key.is_empty()
                    && accepted_key.starts_with(&candidate_key)
            };
            let optimistic_owner_recovery_safe = has_final
                && spoken_content_len(optimistic_text) > spoken_content_len(target_text)
                // Display permission and display length are not ownership
                // evidence. Only an optimistic candidate already contained
                // in the accepted session ledger may use this compatibility
                // path; a newly appended provisional tail stays provisional.
                && optimistic_candidate_is_already_accepted
                && local_speaker_allows_non_destructive_final_recovery(&state);
            let session_ledger_candidate = session_committed_transcript(&state);
            let session_ledger_recovery_safe = has_final
                && !explicit_non_owner_tail
                && session_ledger_candidate
                    .as_ref()
                    .is_some_and(|(ledger_text, _)| {
                        spoken_content_len(ledger_text) > spoken_content_len(target_text)
                            && (two_pass_empty_final
                                || target_text.trim().is_empty()
                                || !authoritative_final_supersedes_repeated_streaming_ledger(
                                    target_text,
                                    ledger_text,
                                ))
                    });
            let evidence = crate::speech_decision_kernel::FinalTranscriptEvidence {
                protocol_final: has_final,
                explicit_non_owner_tail,
                provider_raw_recovery_safe,
                provider_owner_recovery_safe,
                session_ledger_recovery_safe,
                optimistic_owner_recovery_safe,
            };
            let authority = crate::speech_decision_kernel::arbitrate_final_transcript(evidence);
            // The final arbiter is the authoritative owner-boundary decision.
            // Propagate its foreign-tail veto to the coordinator before the
            // final oneshot resolves; otherwise product arbitration can see
            // `filter_required=false` and select the raw provider text again.
            #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
            if explicit_non_owner_tail {
                self.target_speaker_filter_required.store(
                    target_speaker_filter_required_after_final(
                        self.target_speaker_filter_required.load(Ordering::SeqCst),
                        true,
                    ),
                    Ordering::SeqCst,
                );
            }
            if has_final {
                log::info!(
                    "[asr] final arbitration authority={} provider={} filtered={} best={} optimistic={} last_preview={} tracking={} explicit_non_owner_tail={} raw_recovery={} owner_recovery={} ledger_recovery={} optimistic_recovery={}",
                    authority.label(),
                    result
                        .get("text")
                        .and_then(Value::as_str)
                        .map_or(0, spoken_content_len),
                    spoken_content_len(target_text),
                    spoken_content_len(&state.best_transcript_text),
                    spoken_content_len(&state.optimistic_preview_text),
                    spoken_content_len(&state.last_emitted_preview_text),
                    state.local_speaker_tracking_enabled,
                    explicit_non_owner_tail,
                    provider_raw_recovery_safe,
                    provider_owner_recovery_safe,
                    session_ledger_recovery_safe,
                    optimistic_owner_recovery_safe,
                );
            }
            (
                authority,
                explicit_non_owner_tail,
                provider_raw_recovery_safe,
                provider_owner_recovery_safe,
                session_ledger_recovery_safe,
                optimistic_owner_recovery_safe,
                session_ledger_candidate,
            )
        };
        if has_final {
            let ledger_chars = session_ledger_candidate
                .as_ref()
                .map_or(0, |(text, _)| spoken_content_len(text));
            let target_chars = speaker_filtered_result
                .result
                .get("text")
                .and_then(Value::as_str)
                .map_or(0, spoken_content_len);
            let optimistic_chars = speaker_filtered_result
                .optimistic_result
                .get("text")
                .and_then(Value::as_str)
                .map_or(0, spoken_content_len);
            let provider_chars = result
                .get("text")
                .and_then(Value::as_str)
                .map_or(0, spoken_content_len);
            self.record_diagnostic_trace(
                trace_frame,
                true,
                "final_arbitration",
                &format!(
                    "authority={} provider_chars={} filtered_chars={} optimistic_chars={} ledger_chars={} explicit_non_owner={} raw_recovery={} owner_recovery={} ledger_recovery={} optimistic_recovery={}",
                    final_authority.label(),
                    provider_chars,
                    target_chars,
                    optimistic_chars,
                    ledger_chars,
                    explicit_non_owner_tail,
                    provider_raw_recovery_safe,
                    provider_owner_recovery_safe,
                    session_ledger_recovery_safe,
                    optimistic_owner_recovery_safe,
                ),
            );
        }
        // 2026-09-22 arbitration floor: in a wake-accepted (tracked) session the
        // provider stream follows the wake speaker, and when it transcribed a
        // substantial body the session must not be delivered empty just because
        // the local voiceprint filter — amid room interference — kept only the
        // wake phrase. Live case 7f3955c2 (2026-09-22 10:08): provider 86 chars,
        // filtered 4 ("开始录音。") -> "没有识别到语音" after 18 s of dictation.
        // Deliver the provider text instead. Kill-switch env for rollout.
        let arbitration_floor_applied = Self::arbitration_floor_should_override(
            &final_authority,
            self.state.lock().local_speaker_tracking_enabled,
            result
                .get("text")
                .and_then(Value::as_str)
                .map_or(0, spoken_content_len),
            speaker_filtered_result
                .result
                .get("text")
                .and_then(Value::as_str)
                .map_or(0, spoken_content_len),
            session_ledger_candidate
                .as_ref()
                .map_or(0, |(text, _)| spoken_content_len(text)),
            speaker_filtered_result
                .optimistic_result
                .get("text")
                .and_then(Value::as_str)
                .map_or(0, spoken_content_len),
        );
        if arbitration_floor_applied {
            log::info!(
                "[asr] arbitration floor applied: provider body kept={} local filter kept only wake remnant; delivering provider text (explicit_non_owner_tail={})",
                result
                    .get("text")
                    .and_then(Value::as_str)
                    .map_or(0, spoken_content_len),
                explicit_non_owner_tail
            );
            self.record_diagnostic_trace(
                trace_frame,
                true,
                "arbitration_floor",
                "delivered=provider reason=wake_remnant_only",
            );
        }
        let selected_result = if arbitration_floor_applied {
            Some(result)
        } else {
            match final_authority {
            crate::speech_decision_kernel::FinalTranscriptAuthority::ProviderRawRecovery
            | crate::speech_decision_kernel::FinalTranscriptAuthority::ProviderOwnerRecovery => {
                Some(result)
            }
            crate::speech_decision_kernel::FinalTranscriptAuthority::SessionLedgerRecovery => None,
            crate::speech_decision_kernel::FinalTranscriptAuthority::OptimisticOwnerRecovery => {
                Some(&speaker_filtered_result.optimistic_result)
            }
            crate::speech_decision_kernel::FinalTranscriptAuthority::SpeakerFiltered => {
                Some(&speaker_filtered_result.result)
            }
            }
        };
        let mut candidate = if matches!(
            final_authority,
            crate::speech_decision_kernel::FinalTranscriptAuthority::SessionLedgerRecovery
        ) {
            let (text, timed_segments) = session_ledger_candidate
                .clone()
                .unwrap_or_else(|| (String::new(), Vec::new()));
            TranscriptCandidate {
                text,
                timed_segments,
                authoritative_cumulative: false,
            }
        } else {
            transcript_candidate_from_result(selected_result.expect("non-ledger final authority"))
        };
        // Punctuation restoration happens before the boundary snapshot: it is
        // a rendering fix for content-identical text (see the helper), never
        // an ownership or shrink decision.
        if has_final {
            let ledger = self.state.lock().best_transcript_text.clone();
            super::volcengine_transcript::restore_punctuation_from_session_ledger(
                &mut candidate.text,
                &ledger,
            );
        }
        // Apply the stop-boundary ceiling while this is still an unsealed
        // candidate. It is a shrink-only ownership normalization, never a
        // second final writer after the authority has been logged and sealed.
        let candidate_before_stop_ceiling = candidate.text.clone();
        let mut stop_ceiling_applied = false;
        if has_final {
            let state = self.state.lock();
            if let Some(ceiling) = owner_preview_safety_ceiling(
                &state,
                &candidate.text,
                explicit_non_owner_tail,
            ) {
                stop_ceiling_applied = true;
                candidate.text = ceiling;
                // The preview is display text rather than a timed provider
                // segment. Discard stale timing before the candidate is sealed.
                candidate.timed_segments.clear();
            }
            // A clean final can lose its timed rows while the already
            // committed owner ledger remains the only usable candidate. Keep
            // that narrow recovery for an empty result. A non-empty
            // speaker-filtered final remains authoritative even when it is
            // shorter than an old preview: copying the whole preview here
            // would resurrect an excluded room-speech suffix.
            if candidate.text.trim().is_empty()
                && state.local_speaker_tracking_enabled
                && !state.owner_isolation_frozen
            {
                if let Some((owner_text, owner_segments)) = session_committed_transcript(&state) {
                    candidate.text = owner_text;
                    candidate.timed_segments = owner_segments;
                }
            }
        }
        if has_final {
            self.record_diagnostic_trace(
                trace_frame,
                true,
                "final_candidate_boundary",
                &format!(
                    "before_chars={} after_chars={} stop_ceiling_applied={}",
                    spoken_content_len(&candidate_before_stop_ceiling),
                    spoken_content_len(&candidate.text),
                    stop_ceiling_applied,
                ),
            );
        }
        let arbitrated_final_content_len = has_final.then(|| spoken_content_len(&candidate.text));

        // 流结束信号只信帧头 flags（lastPacket / negativeSequence）。
        // 之前误把 utterance.definite=true 当成流结束——但那只代表"这一段语音已固化"，
        // 用户可能还在继续说。结果一收到第一个 definite=true 就关掉接收，
        // 后面用户讲的内容全部丢失（实测丢了 9 秒）。
        let authoritative_two_pass = candidate.authoritative_cumulative && !two_pass_empty_final;
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
            // A protocol-final candidate is already sealed by the sole final
            // transcript arbiter above. The generic streaming merge may only
            // process non-final revisions; allowing it to prepend/append the
            // older ledger after sealing was the hidden second writer behind
            // the interference-vs-truncation regression loop.
            let (mut merged, mut segments, untimed_window) = if has_final {
                (
                    candidate.text.clone(),
                    candidate.timed_segments.clone(),
                    String::new(),
                )
            } else {
                merge_filtered_streaming_candidate_with_untimed_window(
                    &state.best_transcript_text,
                    &state.best_transcript_segments,
                    &state.best_untimed_window,
                    candidate,
                    excluded_tail_provider_coverage.as_ref(),
                )
            };
            state.best_untimed_window = untimed_window;
            // Hard multi-speaker isolation: never grow past the owner ceiling
            // while local identity is off the wake speaker.
            if !has_final {
                let (clamped_text, clamped_segments) =
                    clamp_to_owner_isolation_ceiling(&state, merged, segments);
                merged = clamped_text;
                segments = clamped_segments;
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
                if state.best_transcript_text != merged {
                    state.best_transcript_committed_at = Some(Instant::now());
                }
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
        self.record_diagnostic_trace(trace_frame, has_final, "merged_candidate", &full_text);

        if let Some(arbitrated_content_len) = arbitrated_final_content_len {
            let sealed_content_len = spoken_content_len(&full_text);
            debug_assert!(
                sealed_content_len <= arbitrated_content_len,
                "terminal finalization must never expand the arbitrated transcript"
            );
            // r36（2026-09-18 晚）：把封存认证暴露给协调层。speaker_filtered
            // 认证的主轨终稿 = 说话人过滤已切尾的干净本人全文；产品仲裁在
            // "分离稿截断成主轨前缀"时（r36：主轨 51 字完好 vs 分离稿截断
            // 20 字）需要这一位来区分"分离稿在做排除"与"分离稿丢了正文"。
            *self.last_final_seal_authority_label.lock() = Some(final_authority.label());
            log::info!(
                "[asr] final arbitration sealed authority={} explicit_non_owner_tail={} raw_recovery={} owner_recovery={} ledger_recovery={} optimistic_recovery={} arbitrated_chars={} sealed_chars={}",
                final_authority.label(),
                explicit_non_owner_tail,
                provider_raw_recovery_safe,
                provider_owner_recovery_safe,
                session_ledger_recovery_safe,
                optimistic_owner_recovery_safe,
                arbitrated_content_len,
                sealed_content_len,
            );
        }

        // Owner ASR text is still growing while debounced identity remains
        // target: keep the 1s endpoint clock aligned with live speech so
        // mid-sentence embedding dips cannot freeze local_target_end and cut.
        if preview_holds_endpoint {
            let update = {
                let state = self.state.lock();
                        target_speaker_update_from_state(&state, true, false, false)
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
            self.record_diagnostic_trace(
                trace_frame,
                has_final,
                "partial_callback_text",
                &full_text,
            );
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
            self.record_diagnostic_trace(
                trace_frame,
                has_final,
                "authoritative_preview_text",
                &full_text,
            );
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
            let commit = {
                let mut state = self.state.lock();
                let revision_count = state.transcript_evidence.revision_count();
                // Raw provider recovery is an explicit authority of the sole
                // final arbiter. Terminal delivery may claim exactly-once, but
                // it may not run another fallback after the transcript seal.
                let commit = state.transcript_evidence.commit_once(&full_text, false);
                (commit, revision_count)
            };
            let (Some(commit), revision_count) = commit else {
                log::warn!("[asr] duplicate terminal transcript suppressed");
                self.state.lock().is_connected = false;
                *self.audio_tx.lock() = None;
                return false;
            };
            debug_assert_ne!(
                commit.source,
                crate::speech_decision_kernel::CommitSource::ProviderRawFallback,
                "terminal delivery cannot override the sealed final authority"
            );
            log::info!(
                "[asr] sealed terminal transcript delivered source={} chars={} provider_revisions={}",
                commit.source.label(),
                commit.text.chars().count(),
                revision_count
            );
            let duration_ms = self
                .state
                .lock()
                .start
                .map(|s| s.elapsed().as_millis() as u64)
                .unwrap_or(0);
            let transcript = RawTranscript {
                text: commit.text,
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
        let (commit, duration_ms) = {
            let mut st = self.state.lock();
            let owner_candidate = session_committed_transcript(&st)
                .map(|(text, _)| text)
                .unwrap_or_default();
            let allow_raw_fallback = provider_raw_fallback_allowed(&st);
            let commit = st
                .transcript_evidence
                .commit_once(&owner_candidate, allow_raw_fallback);
            let duration_ms = st
                .start
                .map(|s| s.elapsed().as_millis() as u64)
                .unwrap_or(0);
            (commit, duration_ms)
        };
        match commit {
            Some(commit) if !commit.text.trim().is_empty() => {
                log::warn!(
                    "[asr] {}; 使用会话账本兜底（{} 字, source={}）",
                    err,
                    commit.text.chars().count(),
                    commit.source.label()
                );
                self.signal_success(RawTranscript {
                    text: commit.text,
                    duration_ms,
                });
            }
            Some(_) => {
                log::warn!("[asr] stream ended without a qualified transcript: {err}");
                self.signal_error(err);
            }
            None => log::warn!("[asr] duplicate terminal error delivery suppressed"),
        }
        self.mark_audio_delivery_failed(delivery_error);
        self.state.lock().is_connected = false;
        *self.audio_tx.lock() = None;
    }
}

impl AudioConsumer for VolcengineStreamingASR {
    fn consume_pcm_chunk(&self, pcm: &[u8]) {
        self.consume_pcm_chunk_with_source_interval(pcm, self.pipeline_observation(), None, None);
    }

    fn consume_pcm_chunk_with_source(
        &self,
        pcm: &[u8],
        observation: Option<Arc<crate::observability::EmbeddedAudioPipelineObservation>>,
        segment_id: Option<u32>,
    ) {
        self.consume_pcm_chunk_with_source_interval(pcm, observation, segment_id, None);
    }

    fn consume_pcm_chunk_with_source_interval(
        &self,
        pcm: &[u8],
        observation: Option<Arc<crate::observability::EmbeddedAudioPipelineObservation>>,
        segment_id: Option<u32>,
        source_interval: Option<crate::observability::PcmSourceInterval>,
    ) {
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
        // Acquire the queue boundary before state. Finalization/cancellation
        // uses the same order, so it cannot observe an empty queue after
        // sequences have been allocated but before their audio has reached
        // the worker.
        // `None` is an explicit unknown source for this source-aware entry.
        // The legacy consume_pcm_chunk wrapper supplies the current observation
        // before reaching here; never resurrect a stale Arc at this boundary.
        let observation = observation;
        let source_interval = source_interval.filter(|interval| {
            let legal = interval.is_legal_for_source(
                observation.as_ref().map(Arc::as_ref),
                segment_id,
                pcm.len(),
            );
            if !legal {
                observation
                    .as_ref()
                    .map(|observation| observation.mark_source_interval_incomplete());
            }
            legal
        });
        let _delivery_queue_guard = self.delivery_queue_lock.lock();
        let mut st = self.state.lock();
        let chunks: Vec<(
            i32,
            Vec<u8>,
            Option<crate::observability::PcmRange>,
            Vec<AudioSourceShare>,
        )> = {
            if !st.is_connected || st.finishing {
                return;
            }
            if matches!(
                st.audio_delivery_readiness,
                AudioDeliveryReadiness::Failed(_)
            ) {
                drop(st);
                record_asr_unresolved_for_source_shares(
                    self.asr_stream_id(),
                    None,
                    &[AudioSourceShare {
                        observation,
                        segment_id,
                        bytes: pcm.len(),
                        destination_range: None,
                        source_interval,
                    }],
                    pcm.len(),
                );
                return;
            }
            if matches!(
                st.audio_delivery_readiness,
                AudioDeliveryReadiness::Cancelled
            ) {
                return;
            }
            let destination_range = if st.destination_ledger_incomplete {
                None
            } else {
                let destination_range = crate::observability::PcmRange::from_start_and_bytes(
                    st.next_input_destination_offset,
                    pcm.len(),
                );
                if let Some(range) = destination_range {
                    st.next_input_destination_offset = range.end;
                } else {
                    st.destination_ledger_incomplete = true;
                    observation
                        .as_ref()
                        .map(|observation| observation.mark_asr_destination_facts_incomplete());
                }
                destination_range
            };
            if st.pending_audio.is_empty() {
                st.pending_audio_destination_start = destination_range.map(|range| range.start);
            } else if destination_range.is_none() {
                st.pending_audio_destination_start = None;
            }
            st.pending_audio.extend_from_slice(pcm);
            append_audio_source_run(
                &mut st.pending_audio_sources,
                pcm.len(),
                observation.clone(),
                segment_id,
                destination_range,
                source_interval,
            );

            let mut out: Vec<(
                i32,
                Vec<u8>,
                Option<crate::observability::PcmRange>,
                Vec<AudioSourceShare>,
            )> = Vec::new();
            while st.pending_audio.len() >= TARGET_AUDIO_CHUNK_BYTES {
                let frame_destination_range = if st.destination_ledger_incomplete {
                    None
                } else {
                    st.pending_audio_destination_start.and_then(|start| {
                        crate::observability::PcmRange::from_start_and_bytes(
                            start,
                            TARGET_AUDIO_CHUNK_BYTES,
                        )
                    })
                };
                let chunk: Vec<u8> = st.pending_audio.drain(..TARGET_AUDIO_CHUNK_BYTES).collect();
                let seq = st.next_sequence;
                st.next_sequence += 1;
                st.bytes_sent += chunk.len();
                st.bytes_sent_since_response += chunk.len();
                st.frames_sent += 1;
                let shares = drain_audio_source_runs(&mut st.pending_audio_sources, chunk.len());
                st.pending_audio_destination_start = frame_destination_range
                    .and_then(|range| (!st.pending_audio.is_empty()).then_some(range.end));
                out.push((seq, chunk, frame_destination_range, shares));
            }
            out
        };

        if chunks.is_empty() {
            return;
        }
        // Keep source registration, queue publication, and terminal-state
        // observation in one synchronous boundary. The state guard is already
        // held here; never reacquire it while this function owns it.
        let delivery_terminated = !st.is_connected
            || matches!(
                st.audio_delivery_readiness,
                AudioDeliveryReadiness::Failed(_) | AudioDeliveryReadiness::Cancelled
            );
        if delivery_terminated {
            for (sequence, chunk, destination_range, source_shares) in chunks {
                record_asr_queue_rejected_without_queue_for_source_shares(
                    self.asr_stream_id(),
                    sequence,
                    destination_range,
                    &source_shares,
                    chunk.len(),
                );
            }
            return;
        }
        let Some(tx) = self.audio_tx.lock().as_ref().cloned() else {
            for (sequence, chunk, destination_range, source_shares) in chunks {
                record_asr_queue_rejected_without_queue_for_source_shares(
                    self.asr_stream_id(),
                    sequence,
                    destination_range,
                    &source_shares,
                    chunk.len(),
                );
            }
            return;
        };

        let mut chunks = chunks.into_iter();
        while let Some((entry_sequence, entry_chunk, destination_range, source_shares)) =
            chunks.next()
        {
            let entry_bytes = entry_chunk.len();
            // pending_sends 必须在 tx.send 之前 +1：否则 worker 可能先 recv + 发送 +
            // 减 1，把 usize 计数器 underflow。
            let pending_frames = self.pending_sends.fetch_add(1, Ordering::SeqCst) + 1;
            // A retained-audio replay legitimately queues more than eight
            // frames. Dropping any here loses speech and leaves a sequence
            // hole (physical session 40c7061a: expected 10, received 58).
            update_pending_send_high_water(&self.pending_sends_high_water, pending_frames);
            self.queued_audio_observations.lock().insert(
                (self.asr_stream_id(), entry_sequence),
                source_shares.clone(),
            );
            // Register before publishing to the worker. A fast worker can
            // otherwise settle the frame before the queued counter exists.
            record_asr_queued_for_source_shares(
                self.asr_stream_id(),
                entry_sequence,
                destination_range,
                &source_shares,
                entry_bytes,
            );
            if tx.send((entry_sequence, entry_chunk)).is_err() {
                // worker 已退出（cancel / 错误路径里 audio_tx 被 take）。
                // 撤销刚才的 +1，避免 send_last_frame 的 wait 永远等不到 0。
                if self.pending_sends.fetch_sub(1, Ordering::SeqCst) == 1 {
                    self.send_done.notify_waiters();
                }
                self.queued_audio_observations
                    .lock()
                    .remove(&(self.asr_stream_id(), entry_sequence));
                record_asr_queue_rejected_for_source_shares(
                    self.asr_stream_id(),
                    entry_sequence,
                    destination_range,
                    &source_shares,
                    entry_bytes,
                );
                for (unsent_sequence, unsent_chunk, unsent_destination_range, unsent_sources) in
                    chunks
                {
                    record_asr_queue_rejected_without_queue_for_source_shares(
                        self.asr_stream_id(),
                        unsent_sequence,
                        unsent_destination_range,
                        &unsent_sources,
                        unsent_chunk.len(),
                    );
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

    #[test]
    fn pause_early_rolling_prefix_advances_while_later_words_keep_arriving() {
        let t0 = Instant::now();
        let age = Duration::from_millis(1_000);
        let mut revisions = VecDeque::new();
        let first = "开始录音今天讨论";
        let second = "开始录音今天讨论语音输入";
        let third = "开始录音今天讨论语音输入的速度";
        assert_eq!(
            rolling_stable_transcript_prefix(
                &mut revisions,
                first,
                t0,
                t0 + Duration::from_millis(500),
                age,
            ),
            None
        );
        assert_eq!(
            rolling_stable_transcript_prefix(
                &mut revisions,
                second,
                t0 + Duration::from_millis(600),
                t0 + Duration::from_millis(600),
                age,
            ),
            None
        );
        assert_eq!(
            rolling_stable_transcript_prefix(
                &mut revisions,
                third,
                t0 + Duration::from_millis(1_100),
                t0 + Duration::from_millis(1_100),
                age,
            )
            .as_deref(),
            Some(first),
            "the first clause is stable even though the full transcript changed"
        );
        assert_eq!(
            rolling_stable_transcript_prefix(
                &mut revisions,
                "开始录音今天讨论语音输入的速度和准确率",
                t0 + Duration::from_millis(1_600),
                t0 + Duration::from_millis(1_600),
                age,
            )
            .as_deref(),
            Some(second),
            "the next clause becomes eligible without a whole-sentence pause"
        );
        assert_eq!(
            rolling_stable_transcript_prefix(
                &mut revisions,
                "开始录音今天谈论语音输入的速度和准确率",
                t0 + Duration::from_millis(1_800),
                t0 + Duration::from_millis(1_800),
                age,
            )
            .as_deref(),
            Some("开始录音今天"),
            "a cloud correction must shrink the uncommitted stable prefix"
        );
        assert_eq!(
            rolling_stable_transcript_prefix(
                &mut revisions,
                "",
                t0 + Duration::from_millis(1_900),
                t0 + Duration::from_millis(1_900),
                age,
            ),
            None
        );
        assert!(revisions.is_empty());
    }

    #[test]
    fn pause_early_rolling_snapshot_requires_current_owner_after_a_speaker_dip() {
        use crate::speaker_verification::SessionSpeakerClassification::{NonTarget, Target};
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        let now = Instant::now();
        let first = "开始录音今天讨论语音输入";
        let growing = "开始录音今天讨论语音输入的速度和准确率";
        {
            let mut state = asr.state.lock();
            state.best_transcript_text = growing.into();
            state.best_transcript_committed_at = Some(now);
            state.pause_early_recent_revisions.push_back((
                now - Duration::from_secs(2),
                first.into(),
            ));
            state.local_speaker_tracking_enabled = true;
            state.local_wake_owner_verified = true;
            state.local_speaker_stable_target = true;
            state.local_target_confirmed = true;
            state.local_speaker_classification = Some(NonTarget { score: 0.04 });
        }
        assert!(asr
            .pause_early_delivery_ledger_snapshot(Duration::from_secs(1))
            .is_none());
        asr.state.lock().local_speaker_classification = Some(Target { score: 0.48 });
        assert_eq!(
            asr.pause_early_delivery_ledger_snapshot(Duration::from_secs(1))
                .map(|snapshot| snapshot.text),
            Some(first.into())
        );
    }

    #[test]
    fn stable_ledger_snapshot_requires_finishing_stable_clean_ledger() {
        let mut asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: String::new(),
                access_token: String::new(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        // Not finishing / empty ledger → ineligible.
        assert!(asr
            .stable_ledger_final_snapshot(Duration::from_millis(1))
            .is_none());
        {
            let mut st = asr.state.lock();
            st.finishing = true;
            st.best_transcript_text = "开始录音，所以说刚才是什么问题？".into();
            st.best_transcript_committed_at = Some(
                Instant::now()
                    .checked_sub(Duration::from_secs(2))
                    .expect("clock"),
            );
        }
        let snapshot = asr
            .stable_ledger_final_snapshot(EARLY_SEAL_MIN_LEDGER_STABLE)
            .expect("stable clean finishing ledger is sealable");
        assert_eq!(snapshot.text, "开始录音，所以说刚才是什么问题？");
        // Fresh revision inside the stability budget → ineligible.
        {
            let mut st = asr.state.lock();
            st.best_transcript_committed_at = Some(Instant::now());
        }
        assert!(asr
            .stable_ledger_final_snapshot(EARLY_SEAL_MIN_LEDGER_STABLE)
            .is_none());
        // Non-target evidence (interference session) → ineligible even when stable.
        {
            let mut st = asr.state.lock();
            st.best_transcript_committed_at = Some(
                Instant::now()
                    .checked_sub(Duration::from_secs(2))
                    .expect("clock"),
            );
            st.local_non_target_speech_end_ms = Some(4_000);
        }
        assert!(asr
            .stable_ledger_final_snapshot(EARLY_SEAL_MIN_LEDGER_STABLE)
            .is_none());
        // Frozen isolation ceiling → ineligible.
        {
            let mut st = asr.state.lock();
            st.local_non_target_speech_end_ms = None;
            st.owner_isolation_frozen = true;
        }
        assert!(asr
            .stable_ledger_final_snapshot(EARLY_SEAL_MIN_LEDGER_STABLE)
            .is_none());
    }

    #[test]
    fn arbitration_floor_overrides_only_wake_remnant_filtered_finals() {
        use crate::speech_decision_kernel::FinalTranscriptAuthority as Authority;
        // Live shape (7f3955c2, 2026-09-22 10:08): 86-char provider body cut to
        // the 4-char wake remnant by room interference -> floor delivers provider.
        assert!(VolcengineStreamingASR::arbitration_floor_should_override(
            &Authority::SpeakerFiltered,
            true,
            86,
            4,
            4,
            4
        ));
        // A substantial filtered winner keeps the strict arbitration.
        assert!(!VolcengineStreamingASR::arbitration_floor_should_override(
            &Authority::SpeakerFiltered,
            true,
            60,
            42,
            60,
            60
        ));
        // Substantial owner-confirmed ledger/optimistic text routes to those
        // recovery paths instead of the raw provider stream.
        assert!(!VolcengineStreamingASR::arbitration_floor_should_override(
            &Authority::SpeakerFiltered,
            true,
            86,
            4,
            42,
            4
        ));
        assert!(!VolcengineStreamingASR::arbitration_floor_should_override(
            &Authority::SpeakerFiltered,
            true,
            86,
            4,
            4,
            42
        ));
        // Short command bodies are not floor material.
        assert!(!VolcengineStreamingASR::arbitration_floor_should_override(
            &Authority::SpeakerFiltered,
            true,
            8,
            4,
            4,
            4
        ));
        // Other authorities and untracked (non-wake) sessions stay untouched.
        assert!(!VolcengineStreamingASR::arbitration_floor_should_override(
            &Authority::OptimisticOwnerRecovery,
            true,
            86,
            4,
            4,
            4
        ));
        assert!(!VolcengineStreamingASR::arbitration_floor_should_override(
            &Authority::SpeakerFiltered,
            false,
            86,
            4,
            4,
            4
        ));
    }

    #[tokio::test]
    async fn early_seal_prefers_a_prompt_real_final_and_falls_back_to_the_ledger() {
        let mut asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: String::new(),
                access_token: String::new(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        // Prompt real final wins the race (biased select polls the oneshot first).
        let (final_tx, final_rx) = tokio::sync::oneshot::channel();
        *asr.final_rx.lock() = Some(final_rx);
        final_tx
            .send(Ok(RawTranscript {
                text: "真实终稿".into(),
                duration_ms: 1_800,
            }))
            .expect("pre-delivered final");
        let result = asr.await_final_result_with_early_seal().await;
        assert_eq!(result.expect("real final").text, "真实终稿");

        // No final ever arrives; the stable ledger seals after the head start.
        let mut asr2 = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: String::new(),
                access_token: String::new(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        asr2.install_test_final_wait_barrier_with_provider_result(Err(
            VolcengineASRError::NoFinalResult,
        ));
        {
            let mut st = asr2.state.lock();
            st.finishing = true;
            st.best_transcript_text = "开始录音，说完即封。".into();
            st.best_transcript_committed_at = Some(
                Instant::now()
                    .checked_sub(Duration::from_secs(3))
                    .expect("clock"),
            );
        }
        let started = Instant::now();
        let sealed = asr2.await_final_result_with_early_seal().await;
        assert_eq!(
            sealed.expect("stable ledger seals").text,
            "开始录音，说完即封。"
        );
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    fn diagnostic_trace_is_opt_in_bounded_and_unicode_safe() {
        let mut asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: String::new(),
                access_token: String::new(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        asr.record_diagnostic_trace(1, false, "disabled", "本人正文");
        assert!(asr.take_diagnostic_trace().entries.is_empty());
        asr.enable_diagnostic_trace();
        let long_text = "你😀".repeat(DIAGNOSTIC_TRACE_MAX_CHARS);
        for frame in 0..DIAGNOSTIC_TRACE_MAX_ENTRIES + 2 {
            asr.record_diagnostic_trace(frame, false, "test", &long_text);
        }
        let snapshot = asr.take_diagnostic_trace();
        assert_eq!(snapshot.entries.len(), DIAGNOSTIC_TRACE_MAX_ENTRIES);
        assert_eq!(snapshot.entries_dropped, 2);
        assert_eq!(
            snapshot.entries[0].text.chars().count(),
            DIAGNOSTIC_TRACE_MAX_CHARS
        );
        assert!(snapshot.entries.iter().all(|entry| entry.text_truncated));
        let empty = asr.take_diagnostic_trace();
        assert!(empty.entries.is_empty());
        assert_eq!(empty.entries_dropped, 0);
        asr.record_diagnostic_trace(999, true, "short", "你帮我");
        let next = asr.take_diagnostic_trace();
        assert_eq!(next.entries[0].text, "你帮我");
        assert!(!next.entries[0].text_truncated);
    }

    #[test]
    fn no_proxy_matching_covers_exact_hosts_suffixes_and_wildcard() {
        assert!(no_proxy_matches_host("localhost,127.0.0.1", "localhost"));
        assert!(no_proxy_matches_host(".example.com", "api.example.com"));
        assert!(no_proxy_matches_host("*", "anywhere.invalid"));
        assert!(!no_proxy_matches_host("example.com", "example.net"));
        assert!(no_proxy_matches_host("example.com:443", "example.com"));
    }

    #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
    #[test]
    fn target_speaker_final_wait_is_reserved_for_real_interference() {
        assert!(!target_speaker_final_required(false, false, false));
        assert!(!target_speaker_final_required(true, false, false));
        assert!(target_speaker_final_required(false, true, false));
        assert!(target_speaker_final_required(false, false, true));
        assert!(target_speaker_final_required(true, true, false));
    }

    #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
    #[test]
    fn explicit_final_foreign_tail_forces_product_filter_requirement() {
        assert!(!target_speaker_filter_required_after_final(false, false));
        assert!(target_speaker_filter_required_after_final(false, true));
        assert!(target_speaker_filter_required_after_final(true, false));
        assert!(target_speaker_filter_required_after_final(true, true));
    }

    #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
    #[test]
    fn clean_separator_result_disarms_required_filter_without_weakening_fail_closed_paths() {
        assert!(!target_speaker_filter_required_after_finish(true, true));
        assert!(target_speaker_filter_required_after_finish(true, false));
        assert!(!target_speaker_filter_required_after_finish(false, false));
    }

    #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
    #[test]
    fn incomplete_separator_final_uses_explicit_distinct_provider_target_track() {
        let selected = recover_incomplete_separator_final_from_distinct_provider_track(
            Ok(Some(RawTranscript {
                text: "开始录音现在是主人第一句".into(),
                duration_ms: 11_440,
            })),
            Some(RawTranscript {
                text: "开始录音现在是主人第一句现在继续说主人第二句最后这句话也不能丢".into(),
                duration_ms: 11_440,
            }),
        )
        .unwrap()
        .unwrap();

        assert_eq!(
            selected.text,
            "开始录音现在是主人第一句现在继续说主人第二句最后这句话也不能丢"
        );
    }

    #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
    #[test]
    fn complete_separator_final_remains_authoritative_over_provider_track() {
        let selected = recover_incomplete_separator_final_from_distinct_provider_track(
            Ok(Some(RawTranscript {
                text: "开始录音这是主人完整说的话".into(),
                duration_ms: 7_462,
            })),
            Some(RawTranscript {
                text: "开始录音这是主人完整说的话啊".into(),
                duration_ms: 7_462,
            }),
        )
        .unwrap()
        .unwrap();

        assert_eq!(selected.text, "开始录音这是主人完整说的话");
    }

    #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
    #[test]
    fn installed_session_1293_noise_only_separator_collapse_preserves_owner_track() {
        let selected = recover_noise_only_separator_collapse_from_certified_primary(
            Ok(Some(RawTranscript {
                text: "开始录音然后".into(),
                duration_ms: 20_907,
            })),
            Some(RawTranscript {
                text: "甲".repeat(100),
                duration_ms: 20_907,
            }),
            crate::speech_decision_kernel::InterferenceEvidence {
                physical_overlap: true,
                sustained_non_target: false,
                degraded_owner_tail: false,
            },
        )
        .expect("coverage arbitration succeeds")
        .expect("certified owner track survives");

        assert_eq!(spoken_content_len(&selected.text), 100);
    }

    #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
    #[test]
    fn explicit_non_target_evidence_does_not_restore_single_speaker_primary() {
        let separated = "开始录音主人正文";
        let selected = recover_noise_only_separator_collapse_from_certified_primary(
            Ok(Some(RawTranscript {
                text: separated.into(),
                duration_ms: 20_907,
            })),
            Some(RawTranscript {
                text: "旁人".repeat(50),
                duration_ms: 20_907,
            }),
            crate::speech_decision_kernel::InterferenceEvidence {
                physical_overlap: true,
                sustained_non_target: true,
                degraded_owner_tail: false,
            },
        )
        .expect("coverage arbitration succeeds")
        .expect("separated owner remains available");

        assert_eq!(selected.text, separated);
    }

    // r23 2026-09-18 921fe510（TTS 旁人干扰轮，用户反馈"吞标点"）：产品终稿走
    // separated_owner 通道，文本来自"分离后音频的第二次云解码"——归属内容与
    // provider 说话人过滤轨一致，但没有标点、数字写成汉字（"1秒"→"一秒"）。
    // 终稿必须按分离轨确认的边界改用 provider 原文的带标点渲染。
    #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
    #[test]
    fn r23_separated_owner_final_restores_punctuated_provider_rendering() {
        // 该轮 ASR trace 的 speaker_filtered_result final（58 字，含 6 个标点）。
        let provider_track = RawTranscript {
            text: "开始录音，我现在做端点复测。第一部分先完整保留，中间自然停顿大约1秒，然后继续第二部分，最后这句话也必须要完整保留。".into(),
            duration_ms: 15_286,
        };
        // 分离轨二次解码 = 唤醒词 + 无标点正文（"一秒"），即该轮实际上屏内容。
        let separated = RawTranscript {
            text: "开始录音我现在做端点复测第一部分先完整保留中间自然停顿大约一秒然后继续第二部分最后这句话也必须要完整保留".into(),
            duration_ms: 15_286,
        };

        let selected = restore_separated_owner_final_punctuation_from_provider_track(
            Ok(Some(separated)),
            Some(provider_track),
        )
        .expect("punctuation restore arbitration succeeds")
        .expect("separated owner remains available");

        assert_eq!(
            selected.text,
            "开始录音，我现在做端点复测。第一部分先完整保留，中间自然停顿大约1秒，然后继续第二部分，最后这句话也必须要完整保留。"
        );
    }

    // 同一轮 provider 原文还带着旁人尾巴"我看一下这个呢。是100分。我看一下
    // 这个呢。"。恢复标点绝不能把尾巴带回来：切片边界仍以分离轨内容为准
    // （E/F 归属核心，旁人尾巴必须继续被排除）。
    #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
    #[test]
    fn r23_punctuation_restore_keeps_bystander_tail_excluded() {
        // 该轮 ASR trace 的 provider_raw_result final（含旁人尾巴与"100分"）。
        let provider_track = RawTranscript {
            text: "开始录音，我现在做端点复测。第一部分先完整保留，中间自然停顿大约1秒，然后继续第二部分，最后这句话也必须要完整保留。我看一下这个呢。是100分。我看一下这个呢。".into(),
            duration_ms: 15_286,
        };
        let separated = RawTranscript {
            text: "开始录音我现在做端点复测第一部分先完整保留中间自然停顿大约一秒然后继续第二部分最后这句话也必须要完整保留".into(),
            duration_ms: 15_286,
        };

        let selected = restore_separated_owner_final_punctuation_from_provider_track(
            Ok(Some(separated)),
            Some(provider_track),
        )
        .expect("punctuation restore arbitration succeeds")
        .expect("separated owner remains available");

        assert_eq!(
            selected.text,
            "开始录音，我现在做端点复测。第一部分先完整保留，中间自然停顿大约1秒，然后继续第二部分，最后这句话也必须要完整保留。"
        );
    }

    // 分离轨只确认到第一句时，切片停在第一句的句号上：保留已确认部分的原文
    // 标点，其后未确认的 provider 内容不进入终稿（与现行"分离轨边界即终稿
    // 边界"语义一致，只是字符改取 provider 原文）。
    #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
    #[test]
    fn separated_boundary_slices_punctuated_provider_at_owner_extent() {
        let selected = restore_separated_owner_final_punctuation_from_provider_track(
            Ok(Some(RawTranscript {
                text: "开始录音我现在做端点复测".into(),
                duration_ms: 15_286,
            })),
            Some(RawTranscript {
                text: "开始录音，我现在做端点复测。第一部分先完整保留，然后继续。".into(),
                duration_ms: 15_286,
            }),
        )
        .expect("punctuation restore arbitration succeeds")
        .expect("separated owner remains available");

        assert_eq!(selected.text, "开始录音，我现在做端点复测。");
    }

    // 分离轨与 provider 轨内容确实不同（对不齐）时，保持分离轨原文——
    // 恢复标点不得发明新文本，也不得用 provider 轨覆盖真实的分离差异。
    #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
    #[test]
    fn unalignable_separated_owner_final_keeps_separator_text() {
        let separated = "开始录音这是分离轨听到的完全不同内容";
        let selected = restore_separated_owner_final_punctuation_from_provider_track(
            Ok(Some(RawTranscript {
                text: separated.into(),
                duration_ms: 15_286,
            })),
            Some(RawTranscript {
                text: "开始录音频谱完全对不上的一句话。".into(),
                duration_ms: 15_286,
            }),
        )
        .expect("punctuation restore arbitration succeeds")
        .expect("separated owner remains available");

        assert_eq!(selected.text, separated);
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

    #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
    #[test]
    fn wake_phrase_score_spike_does_not_force_separator_for_steady_owner_body() {
        // Installed session 784214e1: the wake phrase scored 0.93/0.84, while
        // the same owner's free-form body stayed steadily around 0.4-0.53.
        // Comparing the tail with the wake-specific peak delayed Done by the
        // 1.3 s separator tail inference even though no non-target or physical
        // overlap evidence existed.
        let evidence = [
            0.935, 0.845, 0.445, 0.464, 0.444, 0.483, 0.486, 0.479, 0.446, 0.412, 0.497, 0.528,
            0.399, 0.409, 0.451, 0.461, 0.465, 0.509, 0.484,
        ]
        .into_iter()
        .enumerate()
        .map(|(index, score)| LocalSpeakerEvidence {
            audio_end_ms: 1_200 + index as u64 * 400,
            classification: if score >= 0.55 {
                crate::speaker_verification::SessionSpeakerClassification::Target { score }
            } else {
                crate::speaker_verification::SessionSpeakerClassification::Uncertain { score }
            },
            stable_target: true,
        })
        .collect::<Vec<_>>();

        assert!(!degraded_owner_tail_suggests_interference(&evidence));
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
    fn default_resource_id_uses_gateway_accepted_bigasr_hourly_quota() {
        assert_eq!(
            VolcengineCredentials::default_resource_id(),
            "volc.bigasr.sauc.duration"
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

    fn uplink_stall_test_asr() -> VolcengineStreamingASR {
        VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        )
    }

    #[test]
    fn uplink_stall_aborts_never_responding_stream_and_signals_replayable_error() {
        let asr = uplink_stall_test_asr();
        let events = Arc::new(ParkingMutex::new(Vec::new()));
        let events_for_callback = Arc::clone(&events);
        asr.set_streaming_event_callback(Some(Arc::new(move |event| {
            events_for_callback.lock().push(event);
        })));
        {
            let mut st = asr.state.lock();
            st.is_connected = true;
            st.response_frames_seen = 0;
            st.start = Some(Instant::now().checked_sub(Duration::from_millis(5_200)).unwrap());
            st.bytes_sent_since_response = 60_000;
        }

        assert!(asr.abort_if_uplink_stalled());
        {
            let st = asr.state.lock();
            assert!(!st.is_connected, "stall abort must close the transport");
        }
        let stalled_error = events
            .lock()
            .iter()
            .any(|event| matches!(event, VolcengineStreamingEvent::Error(err) if err.to_string().contains("uplink stall")));
        assert!(
            stalled_error,
            "stall abort must surface a provider error that permits full-audio replay"
        );

        // 已掐线后不得重复触发。
        assert!(!asr.abort_if_uplink_stalled());

        // 2026-09-22 09:23 形态：连接后云端全聋——零应答帧，以会话起点为钟。
        let deaf = uplink_stall_test_asr();
        {
            let mut st = deaf.state.lock();
            st.is_connected = true;
            st.response_frames_seen = 0;
            st.last_server_response_at = None;
            st.start = Some(
                Instant::now()
                    .checked_sub(Duration::from_millis(6_200))
                    .unwrap(),
            );
            st.bytes_sent_since_response = 96_000;
        }
        assert!(
            deaf.abort_if_uplink_stalled(),
            "never-responded session past the silence budget must abort (cloud 45000081 would fire at 8s)"
        );
        assert!(!deaf.state.lock().is_connected);
    }

    #[test]
    fn uplink_stall_ignores_fresh_responses_and_thin_sent_audio() {
        let asr = uplink_stall_test_asr();
        {
            let mut st = asr.state.lock();
            st.is_connected = true;
            st.response_frames_seen = 1;
            st.last_server_response_at = Some(Instant::now());
            st.bytes_sent_since_response = 60_000;
        }
        assert!(!asr.abort_if_uplink_stalled(), "fresh response = healthy");

        // A healthy socket may receive audio through a pause without any
        // changed ASR result. It must stay open for the resumed clause.
        {
            let mut st = asr.state.lock();
            st.last_server_response_at = Some(
                Instant::now().checked_sub(Duration::from_millis(5_200)).unwrap(),
            );
        }
        assert!(
            !asr.abort_if_uplink_stalled(),
            "prior response prevents a result-silence false abort"
        );
        assert!(asr.state.lock().post_response_gap_logged);

        let stalled_since = Instant::now()
            .checked_sub(Duration::from_millis(6_000))
            .unwrap();
        {
            let mut st = asr.state.lock();
            st.response_frames_seen = 0;
            st.last_server_response_at = None;
            st.start = Some(stalled_since);
            st.bytes_sent_since_response = 12_000;
        }
        assert!(
            !asr.abort_if_uplink_stalled(),
            "thin uplink (<1.5s audio since response) must not abort: silence pauses legitimately stall responses"
        );

        {
            let mut st = asr.state.lock();
            st.bytes_sent_since_response = 60_000;
            st.response_frames_seen = 0;
            st.last_server_response_at = None;
            st.start = None;
        }
        assert!(
            !asr.abort_if_uplink_stalled(),
            "no clock basis (no response, no session start) must not abort"
        );
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
                accelerate_score: 0,
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
        assert_eq!(request["ssd_version"], "200");
        assert_eq!(request["enable_nonstream"], true);
        assert_eq!(request["enable_accelerate_text"], true);
        assert_eq!(request["end_window_size"], SECOND_PASS_END_WINDOW_MS);
        assert_eq!(
            request["force_to_speech_time"],
            SECOND_PASS_FORCE_TO_SPEECH_MS
        );
        assert_eq!(request["accelerate_score"], 10);
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
    fn r2_speakerless_partial_cannot_become_session_ledger_recovery() {
        // Reproduce the accepted r2 qualification shape: the provider emits
        // speakerless rolling partials (`要` -> `他` -> `他就` -> `他就说`),
        // the capsule may show the provisional text, but no local/provider
        // ownership gate ever commits it into best_transcript_text.
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        let revisions = ["要", "他", "他就", "他就说"];
        for (index, text) in revisions.into_iter().enumerate() {
            let payload = serde_json::to_vec(&json!({
                "audio_info": { "duration": 800 + index * 400 },
                "result": {
                    "text": text,
                    "utterances": [{
                        "additions": { "source": "stream" },
                        "definite": false,
                        "start_time": 320,
                        "end_time": 799 + index * 400,
                        "text": text
                    }]
                }
            }))
            .expect("r2 partial serializes");
            let frame = frame::build(
                MessageType::FullServerResponse,
                Flags::None,
                Serialization::Json,
                &payload,
                None,
            );
            assert!(
                asr.handle_frame(&frame),
                "r2 partial {index} must stay live"
            );
        }

        {
            let state = asr.state.lock();
            assert_eq!(state.optimistic_preview_text, "他就说");
            assert!(state.best_transcript_text.is_empty());
            assert!(state.last_partial_text.is_empty());
            assert!(
                session_committed_transcript(&state).is_none(),
                "display-only r2 text must not qualify as session recovery"
            );
        }

        let (tx, mut rx) = oneshot::channel();
        asr.state.lock().final_tx = Some(tx);
        let final_payload = serde_json::to_vec(&json!({
            "audio_info": { "duration": 4_800 },
            "result": {
                "additions": {
                    "source": "two_pass",
                    "two_pass_empty": "true"
                },
                "utterances": [{
                    "additions": {
                        "source": "two_pass",
                        "two_pass_empty": "true"
                    },
                    "definite": true,
                    "end_time": 4_700
                }]
            }
        }))
        .expect("r2 empty final serializes");
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
            .expect("r2 final must resolve")
            .expect("r2 empty final must remain a successful empty seal");
        assert!(
            transcript.text.is_empty(),
            "unqualified optimistic text must not be promoted into final recovery"
        );
    }

    #[test]
    fn r2_optimistic_tail_cannot_override_an_accepted_owner_prefix() {
        use crate::speaker_verification::SessionSpeakerClassification::Target;

        let owner = "主人已接受正文";
        let provisional_tail = "旁路暂态尾巴";
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
            state.local_speaker_tracking_enabled = true;
            state.local_speaker_stable_target = true;
            state.local_target_confirmed = true;
            state.target_speaker_id = Some("0".into());
            state.best_transcript_text = owner.into();
            state.last_partial_text = owner.into();
            state.optimistic_preview_text = format!("{owner}{provisional_tail}");
            state.last_emitted_preview_text = format!("{owner}{provisional_tail}");
            state.local_speaker_evidence = vec![LocalSpeakerEvidence {
                audio_end_ms: 4_000,
                classification: Target { score: 0.72 },
                stable_target: true,
            }];
        }

        let (tx, mut rx) = oneshot::channel();
        asr.state.lock().final_tx = Some(tx);
        let final_payload = serde_json::to_vec(&json!({
            "audio_info": { "duration": 4_800 },
            "result": {
                "text": format!("{owner}{provisional_tail}"),
                "utterances": [
                    {
                        "additions": { "speaker_id": "0", "source": "two_pass" },
                        "definite": true,
                        "start_time": 400,
                        "end_time": 2_900,
                        "text": owner
                    },
                    {
                        "additions": { "source": "stream" },
                        "definite": false,
                        "start_time": 3_200,
                        "end_time": 4_400,
                        "text": provisional_tail
                    }
                ]
            }
        }))
        .expect("r2 optimistic-tail final serializes");
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
            .expect("r2 optimistic-tail final must resolve")
            .expect("accepted owner prefix must remain deliverable");
        assert_eq!(transcript.text, owner);
        assert!(
            !transcript.text.contains(provisional_tail),
            "a longer display-only tail must not be upgraded by final recovery"
        );
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
    fn session_ledger_keeps_confirmed_owner_body_after_non_target_tail() {
        let mut manual = SyncState::default();
        // Empty ledger → nothing to commit.
        assert!(session_committed_transcript(&manual).is_none());

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
            session_committed_transcript(&manual).map(|(text, _)| text),
            Some("这是应该保留的本人正文".into())
        );
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
    fn speaker_filter_requires_identity_and_excludes_other_people() {
        // E/F contract: without an identity source the filter never anchors to
        // the first stable utterance. Once a session is anchored (wake phrase
        // or local evidence), the target speaker's rows survive and other
        // stable speakers are excluded.
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
        let mut unanchored = None;
        let abstained = filter_result_to_target_speaker(&result, &mut unanchored);
        assert!(
            unanchored.is_none(),
            "no identity source means no anonymous first-speaker anchor"
        );
        assert_eq!(abstained.result["text"], "");

        let mut target = Some("1".to_string());

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
        let mut target = Some("0".to_string());

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
    fn overlapping_stream_row_does_not_duplicate_provider_cumulative_tail() {
        // Live session f9b0b93d: the two-pass row already contained the
        // stream row's final words. Concatenating both rows produced a longer
        // optimistic preview than result.text and pasted the words twice.
        let result = json!({
            "text": "开始录音，现在继续检查效率至上",
            "utterances": [
                {
                    "additions": { "speaker_id": "0", "source": "two_pass" },
                    "definite": true,
                    "start_time": 100,
                    "end_time": 3300,
                    "text": "开始录音，现在继续检查效率至上"
                },
                {
                    "additions": { "source": "stream" },
                    "definite": false,
                    "start_time": 2800,
                    "end_time": 3400,
                    "text": "效率至上"
                }
            ]
        });
        let mut target = Some("0".to_string());
        let filtered = filter_result_to_target_speaker(&result, &mut target);
        assert_eq!(
            filtered.optimistic_result["text"],
            "开始录音，现在继续检查效率至上"
        );

        // If those words were actually spoken twice, result.text contains
        // both occurrences and the filter must preserve both.
        let repeated = json!({
            "text": "开始录音，现在继续检查效率至上效率至上",
            "utterances": [
                {
                    "additions": { "speaker_id": "0", "source": "two_pass" },
                    "definite": true,
                    "start_time": 100,
                    "end_time": 3300,
                    "text": "开始录音，现在继续检查效率至上"
                },
                {
                    "additions": { "source": "stream" },
                    "definite": false,
                    "start_time": 3400,
                    "end_time": 4000,
                    "text": "效率至上"
                }
            ]
        });
        let filtered = filter_result_to_target_speaker(&repeated, &mut target);
        assert_eq!(
            filtered.optimistic_result["text"],
            "开始录音，现在继续检查效率至上效率至上"
        );

        // A later distinct row must remain visible even if the cumulative
        // result.text has not yet caught up with its second occurrence.
        let mut delayed_raw = repeated;
        delayed_raw["text"] = json!("开始录音，现在继续检查效率至上");
        let filtered = filter_result_to_target_speaker(&delayed_raw, &mut target);
        assert_eq!(
            filtered.optimistic_result["text"],
            "开始录音，现在继续检查效率至上效率至上"
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
        let mut target = Some("0".to_string());
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
    fn split_prefix_recovery_requires_sequential_local_owner_evidence() {
        use crate::speaker_verification::SessionSpeakerClassification::{NonTarget, Uncertain};
        for invalid in [
            "missing_local",
            "non_target",
            "overlap",
            "unstable",
            "same_cluster_excluded",
        ] {
            let mut body = json!({"text": "正文", "start_time": 3000, "end_time": 5000,
                "definite": true, "additions": {"speaker_id": "1", "source": "two_pass"}});
            if invalid == "overlap" {
                body["start_time"] = json!(900);
            }
            if invalid == "unstable" {
                body["definite"] = json!(false);
            }
            if invalid == "same_cluster_excluded" {
                body["additions"]["speaker_id"] = json!("0");
            }
            let wake = json!({"text": "开始录音", "start_time": 100, "end_time": 1000,
                "definite": true, "additions": {"speaker_id": "0", "source": "two_pass"}});
            let result = json!({"text": "开始录音正文", "utterances": [wake.clone(), body]});
            let state = SyncState {
                local_speaker_tracking_enabled: true,
                target_speaker_id: Some("0".into()),
                wake_speaker_phrase: Some("开始录音".into()),
                local_speaker_evidence: if invalid == "missing_local" {
                    vec![]
                } else {
                    vec![LocalSpeakerEvidence {
                        audio_end_ms: 4000,
                        stable_target: true,
                        classification: if invalid == "non_target" {
                            NonTarget { score: 0.15 }
                        } else {
                            Uncertain { score: 0.4 }
                        },
                    }]
                },
                ..Default::default()
            };
            let mut filtered = SpeakerFilteredResult {
                result: json!({"text": "开始录音", "utterances": [wake]}),
                optimistic_result: json!({"text": "开始录音"}),
                speaker_info_present: true,
                response_local_body_alias_present: false,
                stable_non_target_utterance_present: false,
                stable_other_speaker_present: true,
                stable_unresolved_speaker_present: false,
                target_speech_end_ms: Some(1000),
                wake_target_speech_end_ms: Some(1000),
                stable_attributed_speech_end_ms: Some(5000),
                pending_unattributed_text: String::new(),
            };
            recover_locally_supported_owner_prefix(&state, &result, &mut filtered);
            assert_eq!(filtered.result["text"], "开始录音", "{invalid}");
            assert!(!filtered.response_local_body_alias_present, "{invalid}");
        }
    }

    #[test]
    #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
    fn quiet_tail_scores_cannot_force_interference_but_voiced_tail_still_can() {
        use crate::speaker_verification::SessionSpeakerClassification::{Target, Uncertain};
        let mut evidence = (0..12)
            .map(|i| LocalSpeakerEvidence {
                audio_end_ms: 1200 + i * 400,
                classification: Target { score: 0.66 },
                stable_target: true,
            })
            .collect::<Vec<_>>();
        let mut unreliable = Vec::new();
        for (i, score) in [0.11666909, 0.13432643, 0.11299123, 0.14871438]
            .into_iter()
            .enumerate()
        {
            let audio_end_ms = 6000 + i as u64 * 400;
            unreliable.push(audio_end_ms);
            evidence.push(LocalSpeakerEvidence {
                audio_end_ms,
                classification: Uncertain { score },
                stable_target: true,
            });
        }
        assert!(
            degraded_owner_tail_suggests_interference(&evidence),
            "old score-only path mistakes quiet floor for interference"
        );
        assert!(!degraded_owner_tail_suggests_interference_with_quality(
            &evidence,
            &unreliable
        ));
        assert!(
            degraded_owner_tail_suggests_interference_with_quality(&evidence, &[]),
            "same drop in adequately voiced windows remains interference evidence"
        );
        assert!(
            !target_speaker_final_required(true, false, false),
            "residual energy alone retains the existing non-blocking policy"
        );
        assert!(
            target_speaker_final_required(false, true, false),
            "sustained non-target identity remains independent"
        );
    }

    #[test]
    fn speaker_signal_quality_survives_transport_recovery_without_losing_early_rows() {
        use crate::speaker_verification::SessionSpeakerClassification::Uncertain;
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        asr.note_local_speaker_tracking_started("开始录音");
        for i in 0..109 {
            asr.note_local_speaker_observation_with_quality(
                1200 + i as u64 * 400,
                Uncertain { score: 0.1 },
                crate::speech_decision_kernel::TranscriptSpeakerEvidence::Inconclusive,
                false,
            );
        }
        let state = asr.state.lock();
        assert_eq!(state.local_speaker_evidence.len(), 109);
        assert_eq!(state.local_speaker_evidence[0].audio_end_ms, 1_200);
        assert_eq!(state.local_unreliable_speaker_evidence_end_ms.len(), 109);
        let snapshot = OwnerContinuitySnapshot::capture(&state);
        let mut restored = SyncState::default();
        snapshot.restore(&mut restored);
        assert_eq!(
            restored.local_unreliable_speaker_evidence_end_ms,
            state.local_unreliable_speaker_evidence_end_ms
        );
        assert_eq!(
            restored.local_speaker_evidence.len(),
            state.local_speaker_evidence.len(),
            "quality cannot discard identity history or audio"
        );
        drop(state);
        asr.note_local_speaker_tracking_started("开始录音");
        assert!(asr
            .state
            .lock()
            .local_unreliable_speaker_evidence_end_ms
            .is_empty());
    }

    #[test]
    fn later_foreign_cluster_cannot_discard_an_earlier_locally_supported_body() {
        use crate::speaker_verification::SessionSpeakerClassification::{Target, Uncertain};
        for (wake, body, foreign) in [
            (
                "开始录音",
                "帮我看一下，现在感觉好像是越来越差了。",
                "旁边的人正在讨论今晚吃什么。",
            ),
            (
                "start recording",
                "Keep the complete original sentence.",
                "An unrelated conversation follows.",
            ),
        ] {
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
                state.local_speaker_tracking_enabled = true;
                state.local_speaker_stable_target = true;
                state.local_speaker_profile_adaptive = true;
                state.target_speaker_id = Some("0".into());
                state.wake_speaker_phrase = Some(wake.into());
                state.local_non_target_speech_end_ms = Some(12_000);
                state.local_sustained_non_target_speech_end_ms = Some(12_000);
                // A short wake is a good match to itself. The following owner
                // body is compatible across phrases, while the later speaker
                // has a separate, time-aligned local departure.
                state.local_speaker_evidence = vec![
                    LocalSpeakerEvidence {
                        audio_end_ms: 1600,
                        classification: Target { score: 0.82 },
                        stable_target: true,
                    },
                    LocalSpeakerEvidence {
                        audio_end_ms: 4400,
                        classification: Uncertain { score: 0.43 },
                        stable_target: true,
                    },
                    LocalSpeakerEvidence {
                        audio_end_ms: 4800,
                        classification: Uncertain { score: 0.36 },
                        stable_target: true,
                    },
                    LocalSpeakerEvidence {
                        audio_end_ms: 5200,
                        classification: Uncertain { score: 0.33 },
                        stable_target: true,
                    },
                    LocalSpeakerEvidence {
                        audio_end_ms: 5600,
                        classification: Uncertain { score: 0.25 },
                        stable_target: true,
                    },
                    LocalSpeakerEvidence {
                        audio_end_ms: 6000,
                        classification: Uncertain { score: 0.37 },
                        stable_target: true,
                    },
                    LocalSpeakerEvidence {
                        audio_end_ms: 12000,
                        classification: Uncertain { score: 0.18 },
                        stable_target: true,
                    },
                ];
            }
            let payload = serde_json::to_vec(&json!({
                "audio_info": {"duration": 12844},
                "result": {"text": format!("{wake}{body}{foreign}"), "utterances": [
                    {"text": wake, "start_time": 160, "end_time": 2062, "definite": true, "additions": {"speaker_id": "0", "source": "two_pass"}},
                    {"text": body, "start_time": 3782, "end_time": 5862, "definite": true, "additions": {"speaker_id": "1", "source": "two_pass"}},
                    {"text": foreign, "start_time": 6542, "end_time": 12842, "definite": true, "additions": {"speaker_id": "2", "source": "two_pass"}}
                ]}
            })).unwrap();
            let packet = frame::build(
                MessageType::FullServerResponse,
                Flags::LastPacket,
                Serialization::Json,
                &payload,
                None,
            );
            asr.handle_frame(&packet);
            assert_eq!(
                asr.state.lock().best_transcript_text,
                format!("{wake}{body}")
            );
        }
    }

    #[test]
    fn calibrated_long_wake_body_does_not_recover_a_short_different_speaker_tail() {
        use crate::speaker_verification::SessionSpeakerClassification::{Target, Uncertain};
        for (wake, body, tail) in [
            (
                "开始录音",
                "请检查连接并保留完整的正文。",
                "后面的另一条指令。",
            ),
            (
                "start recording",
                "Please keep the complete original sentence.",
                "Another instruction follows.",
            ),
        ] {
            let owner = format!("{wake}{body}");
            let result = json!({"text": format!("{owner}{tail}"), "utterances": [
                {"text": owner, "start_time": 200, "end_time": 19800, "definite": true, "additions": {"speaker_id": "0", "source": "two_pass"}},
                {"text": tail, "start_time": 19800, "end_time": 20271, "definite": true, "additions": {"speaker_id": "1", "source": "two_pass"}}
            ]});
            let mut state = SyncState {
                local_speaker_tracking_enabled: true,
                local_speaker_stable_target: true,
                local_target_confirmed: true,
                local_speaker_classification: Some(Uncertain { score: 0.15 }),
                target_speaker_id: Some("0".into()),
                wake_speaker_phrase: Some(wake.into()),
                last_emitted_preview_text: owner.clone(),
                last_emitted_visual_preview_text: owner.clone(),
                best_transcript_text: owner.clone(),
                local_speaker_evidence: vec![
                    LocalSpeakerEvidence {
                        audio_end_ms: 1500,
                        classification: Target { score: 0.93 },
                        stable_target: true,
                    },
                    LocalSpeakerEvidence {
                        audio_end_ms: 5000,
                        classification: Target { score: 0.72 },
                        stable_target: true,
                    },
                    LocalSpeakerEvidence {
                        audio_end_ms: 20200,
                        classification: Uncertain { score: 0.32 },
                        stable_target: true,
                    },
                    LocalSpeakerEvidence {
                        audio_end_ms: 20600,
                        classification: Uncertain { score: 0.32 },
                        stable_target: true,
                    },
                    LocalSpeakerEvidence {
                        audio_end_ms: 21000,
                        classification: Uncertain { score: 0.21 },
                        stable_target: true,
                    },
                    LocalSpeakerEvidence {
                        audio_end_ms: 21400,
                        classification: Uncertain { score: 0.12 },
                        stable_target: true,
                    },
                    LocalSpeakerEvidence {
                        audio_end_ms: 21800,
                        classification: Uncertain { score: 0.15 },
                        stable_target: true,
                    },
                ],
                ..SyncState::default()
            };
            assert!(provider_has_locally_excluded_later_utterance(
                &state, &result
            ));
            state.local_speaker_evidence[1].classification = Uncertain { score: 0.28 };
            assert!(
                !provider_has_locally_excluded_later_utterance(&state, &result),
                "the wake match alone cannot calibrate body exclusion"
            );
            state.local_speaker_evidence[1].classification = Target { score: 0.72 };
            state
                .local_speaker_evidence
                .last_mut()
                .unwrap()
                .classification = Target { score: 0.64 };
            assert!(
                !provider_has_locally_excluded_later_utterance(&state, &result),
                "positive owner continuation wins over a cluster split"
            );
            state
                .local_speaker_evidence
                .last_mut()
                .unwrap()
                .classification = Uncertain { score: 0.15 };
            let last_end = state.local_speaker_evidence.last().unwrap().audio_end_ms;
            state
                .local_speaker_evidence
                .last_mut()
                .unwrap()
                .audio_end_ms += 5000;
            assert!(
                !provider_has_locally_excluded_later_utterance(&state, &result),
                "distant later noise is not identity evidence for this row"
            );
            state
                .local_speaker_evidence
                .last_mut()
                .unwrap()
                .audio_end_ms = last_end;

            let asr = VolcengineStreamingASR::new(
                VolcengineCredentials {
                    app_id: "app".into(),
                    access_token: "token".into(),
                    resource_id: VolcengineCredentials::default_resource_id().into(),
                },
                Vec::new(),
            );
            *asr.state.lock() = state;
            let visible = Arc::new(ParkingMutex::new(Vec::new()));
            let partials = Arc::clone(&visible);
            asr.set_partial_transcript_callback(Some(Arc::new(move |text| {
                partials.lock().push(text)
            })));
            let visuals = Arc::clone(&visible);
            asr.set_visual_partial_transcript_callback(Some(Arc::new(move |text| {
                visuals.lock().push(text)
            })));
            let payload =
                serde_json::to_vec(&json!({"audio_info": {"duration": 22000}, "result": result}))
                    .unwrap();
            let response = |flag| {
                frame::build(
                    MessageType::FullServerResponse,
                    flag,
                    Serialization::Json,
                    &payload,
                    None,
                )
            };
            assert!(asr.handle_frame(&response(Flags::None)));
            // 2026-09-22 拍板：预览默认假设有干扰。身份不确定（Uncertain 带）
            // 的尾巴允许进 display-only 胶囊，不再压黑；归属仍由终稿仲裁执行
            // （下方 LastPacket 断言保持 owner-only）。
            assert!(
                visible.lock().iter().any(|text| text.contains(tail)),
                "identity-uncertain tail must stay visible in preview: {:?}",
                visible.lock()
            );
            let (tx, mut rx) = oneshot::channel();
            asr.state.lock().final_tx = Some(tx);
            assert!(!asr.handle_frame(&response(Flags::LastPacket)));
            assert_eq!(
                rx.try_recv().unwrap().unwrap().text,
                owner,
                "final recovery must obey the same boundary as preview"
            );
        }
    }

    #[test]
    fn pending_identity_change_previews_and_is_not_a_permanent_exclusion() {
        use crate::speaker_verification::SessionSpeakerClassification::{Target, Uncertain};
        for (wake, body, tail) in [
            ("开始录音", "请检查今天的工作安排。", "背景里的另一句话"),
            (
                "start recording",
                "Please check the keyboard connection.",
                "A different voice follows",
            ),
        ] {
            let owner = format!("{wake}{body}");
            let provider = json!({"text": format!("{owner}{tail}"), "utterances": [
                {"text": owner, "start_time": 600, "end_time": 13000, "definite": true, "additions": {"speaker": "0"}},
                {"text": tail, "start_time": 13000, "end_time": 14200, "definite": false}
            ]});
            let mut state = SyncState {
                local_speaker_tracking_enabled: true,
                local_wake_owner_verified: true,
                local_speaker_stable_target: true,
                local_preview_foreign_hint_end_ms: Some(13100),
                local_speaker_classification: Some(Uncertain { score: 0.18 }),
                wake_speaker_phrase: Some(wake.into()),
                last_emitted_preview_text: owner.clone(),
                last_emitted_visual_preview_text: owner.clone(),
                ..SyncState::default()
            };
            assert!(provider_has_pending_speaker_change(&state, &provider));
            // 2026-09-22 拍板：身份待定不再压黑 display-only 预览（归属只在终稿
            // 仲裁执行）；该函数仍不得产生"永久排除"副作用。
            assert_eq!(
                display_only_provisional_preview_candidate(&mut state, &provider, true),
                Some(format!("{owner}{tail}"))
            );
            assert!(state.last_emitted_visual_preview_text.contains(tail));
            assert!(
                !state.local_preview_exclusion_seen,
                "waiting is not a final exclusion verdict"
            );

            let mut settled = provider.clone();
            settled["utterances"][1]["definite"] = json!(true);
            assert!(
                provider_has_pending_speaker_change(&state, &settled),
                "stable text without identity is still pending"
            );
            settled["utterances"][1]["additions"]["speaker"] = json!("0");
            assert!(
                !provider_has_pending_speaker_change(&state, &settled),
                "stable rows go through the existing owner filter"
            );

            state.local_speaker_evidence.push(LocalSpeakerEvidence {
                audio_end_ms: 14000,
                classification: Target { score: 0.65 },
                stable_target: true,
            });
            assert!(!provider_has_pending_speaker_change(&state, &provider));
            // 主人证据回归后预览不回缩（第二次调用因"无新增"去重返回 None，
            // 已显示的完整文本保持原样）。
            assert!(
                display_only_provisional_preview_candidate(&mut state, &provider, true).is_none()
            );
            assert!(state.last_emitted_visual_preview_text.contains(tail));
        }
    }

    #[test]
    fn correlated_handoff_freezes_all_endpoint_growth_until_two_owner_windows_return() {
        use crate::speaker_verification::SessionSpeakerClassification::{Target, Uncertain};

        let provider = json!({"text": "开始录音。本人正文。旁人新句。", "utterances": [
            {"text": "开始录音。本人正文。", "start_time": 400, "end_time": 20271,
             "definite": true, "additions": {"speaker": "0"}},
            {"text": "旁人新句。", "start_time": 20271, "end_time": 22100,
             "definite": false}
        ]});
        let mut state = SyncState {
            local_speaker_tracking_enabled: true,
            local_wake_owner_verified: true,
            local_speaker_profile_adaptive: false,
            local_speaker_stable_target: true,
            local_target_confirmed: true,
            local_audio_duration_ms: Some(22_100),
            local_speech_end_ms: Some(22_100),
            local_target_speech_end_ms: Some(20_800),
            local_preview_foreign_hint_end_ms: Some(21_600),
            local_speaker_classification: Some(Uncertain { score: 0.46 }),
            wake_speaker_phrase: Some("开始录音".into()),
            ..SyncState::default()
        };

        assert!(observe_provider_owner_handoff_candidate(
            &mut state, &provider
        ));
        let frozen = target_speaker_update_from_state(&state, true, true, false);
        assert_eq!(frozen.local_non_target_speech_end_ms, Some(22_100));
        assert!(!frozen.target_activity_advanced);
        assert!(!frozen.pending_activity_advanced);
        assert!(!refresh_local_target_from_owner_preview_activity(
            &mut state
        ));

        assert!(!observe_owner_handoff_recovery(
            &mut state,
            Uncertain { score: 0.49 }
        ));
        assert!(!observe_owner_handoff_recovery(
            &mut state,
            Target { score: 0.59 }
        ));
        assert!(state.local_owner_handoff_suspected);
        assert!(observe_owner_handoff_recovery(
            &mut state,
            Target { score: 0.61 }
        ));
        assert!(!state.local_owner_handoff_suspected);
        assert!(refresh_local_target_from_owner_preview_activity(&mut state));
    }

    #[test]
    fn adaptive_wake_hint_cannot_freeze_endpoint_as_a_persisted_owner_handoff() {
        let provider = json!({"text": "开始录音。正文。新句。", "utterances": [
            {"text": "开始录音。正文。", "start_time": 0, "end_time": 5000,
             "definite": true, "additions": {"speaker": "0"}},
            {"text": "新句。", "start_time": 5000, "end_time": 6200, "definite": false}
        ]});
        let mut state = SyncState {
            local_speaker_tracking_enabled: true,
            local_wake_owner_verified: true,
            local_speaker_profile_adaptive: true,
            local_speaker_stable_target: true,
            local_target_confirmed: true,
            local_preview_foreign_hint_end_ms: Some(5_500),
            wake_speaker_phrase: Some("开始录音".into()),
            ..SyncState::default()
        };
        assert!(!observe_provider_owner_handoff_candidate(
            &mut state, &provider
        ));
        assert!(!state.local_owner_handoff_suspected);
    }

    #[test]
    fn pending_identity_change_covers_visual_optimistic_and_final_paths() {
        use crate::speaker_verification::SessionSpeakerClassification::Uncertain;
        for owner_returns in [false, true] {
            let asr = VolcengineStreamingASR::new(
                VolcengineCredentials {
                    app_id: "app".into(),
                    access_token: "token".into(),
                    resource_id: VolcengineCredentials::default_resource_id().into(),
                },
                Vec::new(),
            );
            let owner = "开始录音我先检查键盘连接，然后确认语音输入。";
            let tail = "后面还有另一句话。";
            let visible = Arc::new(ParkingMutex::new(Vec::new()));
            let partials = Arc::clone(&visible);
            asr.set_partial_transcript_callback(Some(Arc::new(move |text| {
                partials.lock().push(text)
            })));
            let visuals = Arc::clone(&visible);
            asr.set_visual_partial_transcript_callback(Some(Arc::new(move |text| {
                visuals.lock().push(text)
            })));
            // Phrase-accepted sessions may have a weak enrollment match.
            // Preview waiting must also work with that session-local anchor.
            asr.note_local_speaker_tracking_started_with_owner("开始录音", owner_returns);
            {
                let mut state = asr.state.lock();
                state.target_speaker_id = Some("0".into());
                state.last_emitted_preview_text = owner.into();
                state.last_emitted_visual_preview_text = owner.into();
                state.best_transcript_text = owner.into();
            }
            for (end, score) in [(7200, 0.02), (7600, 0.12), (8000, 0.15)] {
                asr.note_local_speaker_observation(
                    end,
                    Uncertain { score },
                    crate::speech_decision_kernel::TranscriptSpeakerEvidence::Inconclusive,
                );
            }
            let mut result = json!({"text": format!("{owner}{tail}"), "utterances": [
                {"text": owner, "start_time": 400, "end_time": 6600, "definite": true, "additions": {"speaker": "0", "source": "two_pass"}},
                {"text": tail, "start_time": 6600, "end_time": 8200, "definite": false, "additions": {"source": "stream"}}
            ]});
            let response = |result: &Value, flag| {
                let payload = serde_json::to_vec(
                    &json!({"audio_info": {"duration": 8500}, "result": result}),
                )
                .unwrap();
                frame::build(
                    MessageType::FullServerResponse,
                    flag,
                    Serialization::Json,
                    &payload,
                    None,
                )
            };
            assert!(asr.handle_frame(&response(&result, Flags::None)));
            // 2026-09-22 拍板：身份待定的尾巴在 visual 通道照样显示；终稿仲裁
            // （下方 LastPacket 断言）才是归属判定点。
            assert!(
                visible.lock().iter().any(|text| text.contains(tail)),
                "identity-uncertain tail must stay visible in preview: {:?}",
                visible.lock()
            );
            assert!(!asr.state.lock().local_preview_exclusion_seen);

            result["utterances"][1]["definite"] = json!(true);
            result["utterances"][1]["additions"]["source"] = json!("two_pass");
            if owner_returns {
                result["utterances"][1]["additions"]["speaker"] = json!("0");
            } else {
                result["text"] = json!(owner);
                result["utterances"][1]["text"] = json!("");
            }
            let (tx, mut rx) = oneshot::channel();
            asr.state.lock().final_tx = Some(tx);
            assert!(!asr.handle_frame(&response(&result, Flags::LastPacket)));
            let final_text = rx.try_recv().unwrap().unwrap().text;
            assert_eq!(
                final_text,
                if owner_returns {
                    format!("{owner}{tail}")
                } else {
                    owner.into()
                }
            );
        }
    }

    #[test]
    fn pending_identity_change_never_holds_initial_body_or_uses_stale_mismatch() {
        let mut state = SyncState {
            local_speaker_tracking_enabled: true,
            local_wake_owner_verified: true,
            local_preview_foreign_hint_end_ms: Some(1300),
            wake_speaker_phrase: Some("开始录音".into()),
            ..SyncState::default()
        };
        let mut provider = json!({"text": "开始录音我的第一句话", "utterances": [
            {"text": "开始录音", "start_time": 0, "end_time": 1300, "definite": true},
            {"text": "我的第一句话", "start_time": 1300, "end_time": 2400, "definite": false}
        ]});
        assert!(!provider_has_pending_speaker_change(&state, &provider));
        provider["utterances"][0]["text"] = json!("开始录音我先说了完整的一句话");
        assert!(provider_has_pending_speaker_change(&state, &provider));
        state.local_preview_foreign_hint_end_ms = None;
        assert!(!provider_has_pending_speaker_change(&state, &provider));
        state.local_preview_foreign_hint_end_ms = Some(1);
        provider["utterances"][1]["start_time"] = json!(4000);
        provider["utterances"][1]["end_time"] = json!(5200);
        assert!(!provider_has_pending_speaker_change(&state, &provider));
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
            local_audio_duration_ms: Some(1_900),
            local_speech_end_ms: Some(1_900),
            local_target_speech_end_ms: Some(1_900),
            local_speaker_classification: Some(
                crate::speaker_verification::SessionSpeakerClassification::Target { score: 0.72 },
            ),
            local_speaker_stable_target: true,
            local_target_confirmed: true,
            local_consecutive_target: 2,
            local_speaker_evidence: vec![LocalSpeakerEvidence {
                audio_end_ms: 1_900,
                classification: crate::speaker_verification::SessionSpeakerClassification::Target {
                    score: 0.72,
                },
                stable_target: true,
            }],
            wake_speaker_phrase: Some("开始录音".into()),
            ..SyncState::default()
        };
        let anchor = OwnerContinuitySnapshot::capture(&configured);
        let mut reset = SyncState::default();

        anchor.restore(&mut reset);

        assert!(reset.local_speaker_tracking_enabled);
        assert!(reset.local_wake_owner_verified);
        assert!(!reset.local_speaker_profile_adaptive);
        assert!(reset.local_speaker_stable_target);
        assert!(reset.local_target_confirmed);
        assert_eq!(reset.local_audio_duration_ms, Some(1_900));
        assert_eq!(reset.local_speech_end_ms, Some(1_900));
        assert_eq!(reset.local_target_speech_end_ms, Some(1_900));
        assert_eq!(reset.local_consecutive_target, 2);
        assert_eq!(reset.local_speaker_evidence.len(), 1);
        assert_eq!(reset.wake_speaker_phrase.as_deref(), Some("开始录音"));

        let disabled = SyncState {
            local_speaker_tracking_enabled: false,
            local_wake_owner_verified: true,
            local_speaker_profile_adaptive: true,
            ..SyncState::default()
        };
        let mut reset_disabled = SyncState::default();
        OwnerContinuitySnapshot::capture(&disabled).restore(&mut reset_disabled);
        assert!(!reset_disabled.local_wake_owner_verified);
        assert!(!reset_disabled.local_speaker_profile_adaptive);
    }

    #[test]
    fn target_speaker_endpoint_transport_reset_keeps_confirmed_owner_continuity() {
        // Live embedded session 2595 classified the wake owner at 1.9 s, then
        // WebSocket startup reset only the watermark while retaining
        // stable_target=true. Every subsequent cross-phrase window was
        // owner-compatible, but the endpoint saw no local owner authority and
        // stopped before the final body words. A transport reset must preserve
        // the entire reducer state, not a subset of its flags.
        let mut before = SyncState {
            local_speaker_tracking_enabled: true,
            local_wake_owner_verified: true,
            local_speaker_stable_target: true,
            local_target_confirmed: true,
            local_audio_duration_ms: Some(1_900),
            local_speech_end_ms: Some(1_900),
            local_target_speech_end_ms: Some(1_900),
            local_speaker_classification: Some(
                crate::speaker_verification::SessionSpeakerClassification::Target { score: 0.57 },
            ),
            wake_speaker_phrase: Some("开始录音".into()),
            ..SyncState::default()
        };
        let anchor = OwnerContinuitySnapshot::capture(&before);

        before.local_audio_duration_ms = None;
        before.local_speech_end_ms = None;
        before.local_target_speech_end_ms = None;
        before.local_speaker_classification = None;
        before.local_target_confirmed = false;
        anchor.restore(&mut before);

        assert_eq!(
            local_owner_continuity(&before),
            LocalOwnerContinuity::Confirmed
        );
        assert_eq!(before.local_target_speech_end_ms, Some(1_900));
        before.local_audio_duration_ms = Some(2_400);
        assert!(refresh_local_target_from_owner_preview_activity(
            &mut before
        ));
        assert_eq!(before.local_target_speech_end_ms, Some(2_400));
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
    fn owner_resuming_after_pause_reopens_visual_preview_past_old_other_boundary() {
        use crate::speaker_verification::SessionSpeakerClassification::{NonTarget, Target};

        let mut state = SyncState {
            local_speaker_tracking_enabled: true,
            local_wake_owner_verified: true,
            local_speaker_stable_target: true,
            local_target_confirmed: true,
            local_speaker_classification: Some(NonTarget { score: 0.05 }),
            local_sustained_non_target_speech_end_ms: Some(11_400),
            qualified_owner_speech_end_ms: Some(9_000),
            last_emitted_preview_text: "开始录音停顿前的句子".into(),
            last_emitted_visual_preview_text: "开始录音停顿前的句子".into(),
            ..SyncState::default()
        };
        let provider = json!({"text": "开始录音停顿前的句子，继续说话后的新内容"});
        assert_eq!(local_owner_continuity(&state), LocalOwnerContinuity::Other);
        assert!(display_only_provisional_preview_candidate(&mut state, &provider, true).is_none());

        // A new qualified owner sample after the quiet gap may restore the
        // display, while the historical non-target boundary remains available
        // to endpointing and final speaker arbitration.
        state.local_owner_absence_run_confirmed = false;
        state.local_speaker_classification = Some(Target { score: 0.45 });
        state.qualified_owner_speech_end_ms = Some(12_500);
        assert_eq!(local_owner_continuity(&state), LocalOwnerContinuity::Confirmed);
        assert_eq!(
            display_only_provisional_preview_candidate(&mut state, &provider, true),
            provider["text"].as_str().map(str::to_string)
        );
        assert_eq!(state.local_sustained_non_target_speech_end_ms, Some(11_400));

        // A later explicit non-owner sample closes the visual channel again.
        state.local_speaker_classification = Some(NonTarget { score: 0.04 });
        let foreign = json!({"text": "开始录音停顿前的句子，继续说话后的新内容，旁人说话"});
        assert!(display_only_provisional_preview_candidate(&mut state, &foreign, true).is_none());
    }

    #[test]
    fn historical_owner_absence_does_not_freeze_a_recovered_uncertain_continuation() {
        use crate::speaker_verification::SessionSpeakerClassification::{NonTarget, Uncertain};

        // 0c67f852: two low windows confirmed owner absence at 7.5 s. The
        // subsequent 0.306..0.34 windows cleared that run, but the historical
        // watermark was kept for final arbitration and accidentally kept the
        // live preview and insertion gate closed until cloud two-pass sealing.
        let mut state = SyncState {
            local_speaker_tracking_enabled: true,
            local_speaker_stable_target: true,
            local_target_confirmed: true,
            local_sustained_non_target_speech_end_ms: Some(7_500),
            qualified_owner_speech_end_ms: Some(1_900),
            local_owner_absence_run_confirmed: false,
            local_speaker_classification: Some(Uncertain { score: 0.34 }),
            last_emitted_preview_text: "开始录音，我感觉".into(),
            ..SyncState::default()
        };
        assert_eq!(local_owner_continuity(&state), LocalOwnerContinuity::Compatible);
        assert!(local_speaker_allows_optimistic_preview(&state));
        let provider = json!({"text": "开始录音，我感觉有时候会卡"});
        assert!(display_only_provisional_preview_candidate(&mut state, &provider, true).is_some());
        assert_eq!(state.local_sustained_non_target_speech_end_ms, Some(7_500));

        // A new low-score window or an explicit NonTarget re-closes the gate.
        state.local_speaker_classification = Some(Uncertain { score: 0.28 });
        assert_eq!(local_owner_continuity(&state), LocalOwnerContinuity::Other);
        state.local_speaker_classification = Some(NonTarget { score: 0.17 });
        assert_eq!(local_owner_continuity(&state), LocalOwnerContinuity::Other);
        assert!(!local_speaker_allows_optimistic_preview(&state));
    }

    #[test]
    fn physical_foreign_provisional_row_previews_but_final_keeps_filtered_owner() {
        use crate::speaker_verification::SessionSpeakerClassification::{Target, Uncertain};
        let owner = "你帮我看一下，现在感觉好像是越来越差了。";
        let visible = format!("{owner}旁边");
        let mut state = SyncState {
            local_speaker_tracking_enabled: true,
            local_speaker_stable_target: true,
            wake_speaker_phrase: Some("开始录音".into()),
            local_speaker_classification: Some(Uncertain { score: 0.28 }),
            last_emitted_preview_text: owner.into(),
            last_emitted_visual_preview_text: visible.clone(),
            local_speaker_evidence: vec![
                LocalSpeakerEvidence {
                    audio_end_ms: 4300,
                    classification: Target { score: 0.61 },
                    stable_target: true,
                },
                LocalSpeakerEvidence {
                    audio_end_ms: 7100,
                    classification: Uncertain { score: 0.245 },
                    stable_target: true,
                },
                LocalSpeakerEvidence {
                    audio_end_ms: 7500,
                    classification: Uncertain { score: 0.245 },
                    stable_target: true,
                },
                LocalSpeakerEvidence {
                    audio_end_ms: 7900,
                    classification: Uncertain { score: 0.281 },
                    stable_target: true,
                },
            ],
            ..SyncState::default()
        };
        let provider = json!({
            "text": format!("{owner}旁边的人正在讨论今晚吃什么"),
            "utterances": [
                {"text": owner, "start_time": 3082, "end_time": 5442, "definite": true, "additions": {"speaker": "0"}},
                {"text": "旁边的人正在讨论今晚吃什么", "start_time": 5522, "end_time": 8200, "definite": false}
            ]
        });
        assert!(provider_has_locally_excluded_later_utterance(
            &state, &provider
        ));
        assert!(
            !provider_has_locally_excluded_later_utterance_with_stability(
                &state, &provider, true,
            ),
            "a speakerless provisional tail is not a permanent foreign verdict"
        );
        assert!(update_local_preview_exclusion(&mut state, &provider));
        assert!(!state.local_preview_exclusion_seen);
        let mut settled_owner = provider.clone();
        settled_owner["utterances"][1]["definite"] = json!(true);
        settled_owner["utterances"][1]["additions"] = json!({ "speaker": "0" });
        assert!(!update_local_preview_exclusion(&mut state, &settled_owner));
        // 2026-09-22 拍板：Uncertain 带（0.245-0.281）不再是压黑证据——预览照常
        // 增长；旁人尾巴由终稿仲裁切除（下方 final 断言保持 owner-only）。
        let full_preview = format!("{owner}旁边的人正在讨论今晚吃什么");
        assert_eq!(
            display_only_provisional_preview_candidate(&mut state, &provider, true),
            Some(full_preview.clone())
        );
        assert_eq!(
            state.last_emitted_visual_preview_text, full_preview,
            "preview grows with the provisional foreign row"
        );

        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        {
            let mut final_state = asr.state.lock();
            final_state.local_preview_exclusion_seen = true;
            final_state.local_speaker_tracking_enabled = true;
            final_state.local_speaker_stable_target = true;
            final_state.local_speaker_classification = Some(Uncertain { score: 0.28 });
            final_state.local_speaker_evidence = state.local_speaker_evidence.clone();
            final_state.wake_speaker_phrase = state.wake_speaker_phrase.clone();
            final_state.target_speaker_id = Some("0".into());
            final_state.best_transcript_text = owner.into();
            final_state.last_emitted_preview_text = visible.clone();
        }
        let (tx, mut rx) = oneshot::channel();
        asr.state.lock().final_tx = Some(tx);
        let final_result = json!({
            "audio_info": {"duration": 8900},
            "result": {"text": format!("{owner}旁边的人正在讨论今晚吃什么"), "utterances": [
                {"text": owner, "start_time": 2852, "end_time": 5442, "definite": true, "additions": {"speaker_id": "0", "source": "two_pass"}},
                {"text": "旁边的人正在讨论今晚吃什么", "start_time": 5502, "end_time": 8902, "definite": true, "additions": {"speaker_id": "1", "source": "two_pass"}}
            ]}
        });
        let payload = serde_json::to_vec(&final_result).unwrap();
        let final_frame = frame::build(
            MessageType::FullServerResponse,
            Flags::LastPacket,
            Serialization::Json,
            &payload,
            None,
        );
        assert!(!asr.handle_frame(&final_frame));
        assert_eq!(
            rx.try_recv().unwrap().unwrap().text,
            owner,
            "protocol final must keep the filtered owner result rather than old preview tail"
        );

        state.local_speaker_evidence.push(LocalSpeakerEvidence {
            audio_end_ms: 8300,
            classification: Target { score: 0.61 },
            stable_target: true,
        });
        assert!(
            !provider_has_locally_excluded_later_utterance(&state, &provider),
            "a positive owner window protects a cross-phrase dip"
        );
        state.local_speaker_evidence.pop();
        state.local_speaker_evidence[0].classification = Uncertain { score: 0.28 };
        assert!(
            !provider_has_locally_excluded_later_utterance(&state, &provider),
            "an uncalibrated body must not become an exclusion authority"
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
    fn post_stop_final_cannot_append_unseen_spoken_tail_to_preview() {
        let state = SyncState {
            finishing: true,
            local_speaker_tracking_enabled: true,
            last_emitted_preview_text: "开始录音。主人正文到这里。".into(),
            ..SyncState::default()
        };
        assert_eq!(
            owner_preview_safety_ceiling(&state, "开始录音。主人正文到这里。旁人插入的字。", true),
            Some("开始录音。主人正文到这里。".into())
        );
    }

    /// 2026-09-23 18:49 会话 5ac70fc7：用户停顿 ~2.6s 后在 3s 窗口内续说，
    /// 端点竞速输了照常 STOP，尾音经 drain 上云，owner 过滤终稿 58 字完整
    /// 含尾巴——旧天花板砍回 45 字把续说全吞。终稿覆盖比 STOP 水位多出
    /// 真实新音频（+4.5s）时必须豁免；无新音频（两遍精修/幻听，覆盖只差
    /// drain 兜底）不豁免。
    #[test]
    fn post_stop_owner_tail_backed_by_new_audio_is_not_capped() {
        let state = SyncState {
            finishing: true,
            local_speaker_tracking_enabled: true,
            last_emitted_preview_text: "开始录音。主人正文到这里。".into(),
            stop_boundary_server_audio_ms: Some(10_500),
            last_server_audio_duration_ms: Some(13_500),
            ..SyncState::default()
        };
        assert!(owner_preview_safety_ceiling(
            &state,
            "开始录音。主人正文到这里。然后我现在短暂停顿，继续说话。",
            true,
        )
        .is_none());

        // 覆盖只差 drain 兜底（<1.5s）＝无新音频，维持砍尾。
        let drain_only = SyncState {
            finishing: true,
            local_speaker_tracking_enabled: true,
            last_emitted_preview_text: "开始录音。主人正文到这里。".into(),
            stop_boundary_server_audio_ms: Some(10_500),
            last_server_audio_duration_ms: Some(11_200),
            ..SyncState::default()
        };
        assert_eq!(
            owner_preview_safety_ceiling(
                &drain_only,
                "开始录音。主人正文到这里。然后我现在短暂停顿，继续说话。",
                true,
            ),
            Some("开始录音。主人正文到这里。".into())
        );

        // 无水位（直发终稿路径）维持保守砍尾。
        let no_watermark = SyncState {
            finishing: true,
            local_speaker_tracking_enabled: true,
            last_emitted_preview_text: "开始录音。主人正文到这里。".into(),
            stop_boundary_server_audio_ms: None,
            last_server_audio_duration_ms: Some(13_500),
            ..SyncState::default()
        };
        assert!(owner_preview_safety_ceiling(
            &no_watermark,
            "开始录音。主人正文到这里。然后我现在短暂停顿，继续说话。",
            true,
        )
        .is_some());
    }

    #[test]
    fn post_stop_final_keeps_punctuation_only_correction() {
        let state = SyncState {
            finishing: true,
            local_speaker_tracking_enabled: true,
            last_emitted_preview_text: "开始录音主人正文".into(),
            ..SyncState::default()
        };
        assert!(owner_preview_safety_ceiling(&state, "开始录音，主人正文。", false).is_none());
    }

    #[test]
    fn final_owner_correction_replaces_foreign_provisional_lead_in_without_losing_punctuation() {
        let state = SyncState {
            finishing: true,
            local_speaker_tracking_enabled: true,
            wake_speaker_phrase: Some("开始录音".into()),
            last_emitted_preview_text: "Her.开始录音，然后确认规划器都能递归吧".into(),
            ..SyncState::default()
        };
        assert_eq!(
            owner_preview_safety_ceiling(
                &state,
                "开始录音，然后确认规划器都能递归吧？Express products.",
                true,
            ),
            Some("开始录音，然后确认规划器都能递归吧？".into()),
        );
        assert!(owner_preview_safety_ceiling(
            &state,
            "开始录音，然后检查规划器都能递归吧？再补一句。",
            false,
        )
        .is_none());
    }

    #[test]
    fn empty_target_filter_does_not_drop_provider_final_without_non_owner_evidence() {
        let state = SyncState {
            local_speaker_tracking_enabled: true,
            ..SyncState::default()
        };
        let filtered = SpeakerFilteredResult {
            result: json!({"text": ""}),
            optimistic_result: json!({"text": ""}),
            speaker_info_present: true,
            response_local_body_alias_present: false,
            stable_non_target_utterance_present: false,
            stable_other_speaker_present: false,
            stable_unresolved_speaker_present: false,
            target_speech_end_ms: None,
            wake_target_speech_end_ms: None,
            stable_attributed_speech_end_ms: None,
            pending_unattributed_text: String::new(),
        };
        assert!(final_unfiltered_provider_recovery_allowed(
            &state,
            &filtered,
            &json!({"text": "主人完整正文"}),
        ));

        let mut isolated = state;
        isolated.owner_isolation_frozen = true;
        assert!(!final_unfiltered_provider_recovery_allowed(
            &isolated,
            &filtered,
            &json!({"text": "旁人正文"}),
        ));

        // No local voiceprint means a single cloud-attributed row must not
        // truncate the complete provider final when no other-speaker evidence
        // exists.
        let untracked_state = SyncState::default();
        let partial_filtered = SpeakerFilteredResult {
            result: json!({"text": "你傻呀！"}),
            optimistic_result: json!({"text": "你傻呀！"}),
            speaker_info_present: true,
            response_local_body_alias_present: false,
            stable_non_target_utterance_present: false,
            // Cloud cluster drift alone is not identity evidence when local
            // voiceprint tracking is unavailable.
            stable_other_speaker_present: true,
            stable_unresolved_speaker_present: false,
            target_speech_end_ms: None,
            wake_target_speech_end_ms: None,
            stable_attributed_speech_end_ms: None,
            pending_unattributed_text: String::new(),
        };
        assert!(final_unfiltered_provider_recovery_allowed(
            &untracked_state,
            &partial_filtered,
            &json!({"text": "你傻呀！今天的工作顺序是先完成设备配对"}),
        ));
    }

    #[test]
    fn stable_other_speaker_is_a_final_foreign_veto_even_without_local_non_target() {
        let state = SyncState {
            local_speaker_tracking_enabled: true,
            target_speaker_id: Some("owner".to_string()),
            ..SyncState::default()
        };
        let filtered = SpeakerFilteredResult {
            result: json!({"text": "主人正文"}),
            optimistic_result: json!({"text": "主人正文旁人干扰"}),
            speaker_info_present: true,
            response_local_body_alias_present: false,
            stable_non_target_utterance_present: false,
            stable_other_speaker_present: true,
            stable_unresolved_speaker_present: false,
            target_speech_end_ms: Some(2_000),
            wake_target_speech_end_ms: Some(500),
            stable_attributed_speech_end_ms: Some(3_000),
            pending_unattributed_text: String::new(),
        };
        let provider = json!({"text": "主人正文旁人干扰"});
        assert!(final_explicit_non_owner_tail(&state, &filtered, &provider));
        assert_eq!(
            crate::speech_decision_kernel::arbitrate_final_transcript(
                crate::speech_decision_kernel::FinalTranscriptEvidence {
                    protocol_final: true,
                    explicit_non_owner_tail: final_explicit_non_owner_tail(
                        &state, &filtered, &provider,
                    ),
                    provider_raw_recovery_safe: true,
                    provider_owner_recovery_safe: true,
                    session_ledger_recovery_safe: true,
                    optimistic_owner_recovery_safe: true,
                },
            ),
            crate::speech_decision_kernel::FinalTranscriptAuthority::SpeakerFiltered
        );
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
        let fallback = session_committed_transcript(&state);
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
    fn media_time_owner_continuation_is_accepted_by_contrast() {
        use crate::speaker_verification::SessionSpeakerClassification::{
            NonTarget, Target, Uncertain,
        };
        // Installed session e57053aa (2026-09-19 22:07Z): with background media
        // playing, the wake anchored normally, the thinking-pause gaps filled
        // with pure media (NonTarget 0.03-0.12), and the owner's resumed
        // sentences only ever reached Uncertain 0.26-0.44 against the session
        // target — below every absolute owner band, and after two media
        // windows the debounce latch (stable_target) broke as well. Absolute
        // scores cannot separate this from a bystander; the session's own
        // interference cluster can.
        let evidence = vec![
            LocalSpeakerEvidence {
                audio_end_ms: 1_200,
                classification: Target { score: 0.61 },
                stable_target: true,
            },
            LocalSpeakerEvidence {
                audio_end_ms: 2_400,
                classification: Uncertain { score: 0.40 },
                stable_target: true,
            },
            LocalSpeakerEvidence {
                audio_end_ms: 4_200,
                classification: NonTarget { score: 0.05 },
                stable_target: true,
            },
            LocalSpeakerEvidence {
                audio_end_ms: 5_000,
                classification: NonTarget { score: 0.12 },
                stable_target: false,
            },
            LocalSpeakerEvidence {
                audio_end_ms: 6_400,
                classification: Uncertain { score: 0.30 },
                stable_target: false,
            },
            LocalSpeakerEvidence {
                audio_end_ms: 7_800,
                classification: Uncertain { score: 0.38 },
                stable_target: false,
            },
            LocalSpeakerEvidence {
                audio_end_ms: 9_200,
                classification: Uncertain { score: 0.44 },
                stable_target: false,
            },
        ];
        let tail_text = "我觉得归根结底是什么问题呢？能不能修一下根因？感觉一直都修不好的样子。";
        // Split-cluster shape: cloud diarization rolled the resumed sentence
        // into a new cluster; no stable owner window exists to vote it in.
        let tail_split_cluster = json!({
            "additions": { "speaker_id": "1", "source": "two_pass" },
            "definite": true,
            "start_time": 5_500,
            "end_time": 9_500,
            "text": tail_text
        });
        assert!(
            utterance_belongs_to_verified_target(
                &tail_split_cluster,
                "0",
                true,
                &evidence,
                Some("开始录音"),
                Some(1_000),
                false,
                true,
                false,
                None,
            ),
            "owner's media-mixed resumed sentence must survive via session contrast"
        );
        // Same-cluster shape: the unstable windows must not be misread as an
        // owner departure either.
        let tail_same_cluster = json!({
            "additions": { "speaker_id": "0", "source": "two_pass" },
            "definite": true,
            "start_time": 5_500,
            "end_time": 9_500,
            "text": tail_text
        });
        assert!(utterance_belongs_to_verified_target(
            &tail_same_cluster,
            "0",
            true,
            &evidence,
            Some("开始录音"),
            Some(1_000),
            false,
            true,
            false,
            None,
        ));
    }

    #[test]
    fn contrastive_continuation_keeps_bystander_shapes_excluded() {
        use crate::speaker_verification::SessionSpeakerClassification::{
            NonTarget, Target, Uncertain,
        };
        let bystander_row = |start_ms: i64, text: &str| {
            json!({
                "additions": { "speaker_id": "1", "source": "two_pass" },
                "definite": true,
                "start_time": start_ms,
                "end_time": start_ms + 3_000,
                "text": text
            })
        };
        // (a) A real second speaker produces their own NonTarget windows
        // against the same-session target: absolute veto inside the row.
        let media_session = vec![
            LocalSpeakerEvidence {
                audio_end_ms: 1_200,
                classification: Target { score: 0.61 },
                stable_target: true,
            },
            LocalSpeakerEvidence {
                audio_end_ms: 4_200,
                classification: NonTarget { score: 0.05 },
                stable_target: true,
            },
            LocalSpeakerEvidence {
                audio_end_ms: 7_500,
                classification: NonTarget { score: 0.11 },
                stable_target: false,
            },
            LocalSpeakerEvidence {
                audio_end_ms: 8_200,
                classification: Uncertain { score: 0.45 },
                stable_target: false,
            },
        ];
        assert!(
            !utterance_belongs_to_verified_target(
                &bystander_row(6_500, "旁边的人插话讨论"),
                "0",
                true,
                &media_session,
                Some("开始录音"),
                Some(1_000),
                false,
                true,
                false,
                None,
            ),
            "a bystander's own NonTarget window must veto the contrast path"
        );
        // (b) Clean session, no interference cluster: duration evidence
        // replaces the contrast baseline. A single overlapping Uncertain
        // window is too little to claim the row...
        let clean_session = vec![
            LocalSpeakerEvidence {
                audio_end_ms: 1_200,
                classification: Target { score: 0.61 },
                stable_target: true,
            },
            LocalSpeakerEvidence {
                audio_end_ms: 7_400,
                classification: Uncertain { score: 0.38 },
                stable_target: false,
            },
        ];
        assert!(
            !utterance_belongs_to_verified_target(
                &bystander_row(6_500, "旁边的人插话讨论"),
                "0",
                true,
                &clean_session,
                Some("开始录音"),
                Some(1_000),
                false,
                true,
                false,
                None,
            ),
            "one Uncertain window is not duration evidence for a clean-room row"
        );
        // ...but two windows at same-speaker cross-phrase dip scores
        // (0.30-0.33 documented band) admit the clause. Installed 7188c773
        // (2026-09-20 08:14, quiet room) lost exactly this shape: "再整体提
        // 交一遍1.0.5呗" sealed as its own cloud row with Uncertain-only
        // windows while both neighbouring clauses survived.
        let clean_owner_dip = vec![
            LocalSpeakerEvidence {
                audio_end_ms: 1_200,
                classification: Target { score: 0.61 },
                stable_target: true,
            },
            LocalSpeakerEvidence {
                audio_end_ms: 7_400,
                classification: Uncertain { score: 0.33 },
                stable_target: true,
            },
            LocalSpeakerEvidence {
                audio_end_ms: 8_200,
                classification: Uncertain { score: 0.30 },
                stable_target: false,
            },
        ];
        assert!(
            utterance_belongs_to_verified_target(
                &bystander_row(6_500, "再整体提交一遍1.0.5呗"),
                "0",
                true,
                &clean_owner_dip,
                Some("开始录音"),
                Some(1_000),
                false,
                true,
                false,
                None,
            ),
            "a quiet-room owner clause dipping to Uncertain must survive on duration evidence"
        );
        // (c) A row that starts before the wake (bystander-first, r17 shape)
        // can never ride the contrast.
        assert!(
            !utterance_belongs_to_verified_target(
                &bystander_row(100, "旁人先开口说话"),
                "0",
                true,
                &media_session,
                Some("开始录音"),
                Some(1_000),
                false,
                true,
                false,
                None,
            ),
            "pre-wake rows must stay excluded"
        );
        // (d) An unverified wake cannot arm the contrast at all.
        assert!(
            !utterance_belongs_to_verified_target(
                &bystander_row(6_500, "随便什么人接着说"),
                "0",
                true,
                &media_session,
                Some("开始录音"),
                Some(1_000),
                false,
                false,
                false,
                None,
            ),
            "the contrast requires the verified wake owner"
        );
    }

    #[test]
    fn phrase_only_wake_with_verified_body_keeps_same_speaker_post_pause_tail() {
        use crate::speaker_verification::SessionSpeakerClassification::{Target, Uncertain};
        // Installed c61bda72: the wake bank missed, but two body windows
        // positively matched the enrolled owner. The provider marked the
        // owner's resumed clause with the same speaker id after a brief
        // speaker-0 interruption. The old wake-only contrast gate dropped it.
        let mut evidence = vec![
            LocalSpeakerEvidence {
                audio_end_ms: 4_000,
                classification: Target { score: 0.458 },
                stable_target: true,
            },
            LocalSpeakerEvidence {
                audio_end_ms: 7_500,
                classification: Target { score: 0.446 },
                stable_target: true,
            },
        ];
        evidence.extend([12_800, 13_200, 14_400, 16_000, 18_000, 19_600]
            .into_iter()
            .map(|audio_end_ms| LocalSpeakerEvidence {
                audio_end_ms,
                classification: Uncertain { score: 0.365 },
                stable_target: true,
            }));
        let resumed_owner = json!({
            "additions": { "speaker_id": "1", "source": "two_pass" },
            "definite": true,
            "start_time": 11_920,
            "end_time": 20_212,
            "text": "就是识别是像最像的，不是要两个人的声纹，没有声纹分离吗？"
        });
        assert!(utterance_belongs_to_verified_target(
            &resumed_owner, "1", true, &evidence, Some("开始录音"),
            Some(11_280), false, false, false, None,
        ));
        let other_speaker = json!({
            "additions": { "speaker_id": "0", "source": "two_pass" },
            "definite": true,
            "start_time": 11_320,
            "end_time": 11_920,
            "text": "小爱同学。"
        });
        assert!(!utterance_belongs_to_verified_target(
            &other_speaker, "1", true, &evidence, Some("开始录音"),
            Some(11_280), false, false, false, None,
        ));
        let weak_continuation = json!({
            "additions": { "speaker_id": "1", "source": "two_pass" },
            "definite": true,
            "start_time": 21_000,
            "end_time": 24_000,
            "text": "其他人的声音"
        });
        evidence.extend([21_800, 22_400, 23_000].into_iter().map(|audio_end_ms| LocalSpeakerEvidence {
            audio_end_ms,
            classification: Uncertain { score: 0.27 },
            stable_target: true,
        }));
        assert!(utterance_belongs_to_verified_target(
            &weak_continuation, "1", true, &evidence, Some("开始录音"),
            Some(11_280), false, false, false, None,
        ));
    }

    #[test]
    fn media_time_owner_tail_survives_final_sequential_split_gate() {
        use crate::speaker_verification::SessionSpeakerClassification::{
            NonTarget, Target, Uncertain,
        };
        // Final-side twin of e57053aa: the identity latch broke during the
        // media gap and the room was still loud at final time, but the split
        // row's own windows carry no second person and clear the session's
        // interference ceiling by a wide margin.
        let mut state = SyncState {
            local_speaker_tracking_enabled: true,
            local_wake_owner_verified: true,
            local_speaker_stable_target: false,
            local_consecutive_non_target: 2,
            target_speaker_id: Some("0".into()),
            wake_speaker_phrase: Some("开始录音".into()),
            ..SyncState::default()
        };
        state.local_speaker_evidence = vec![
            LocalSpeakerEvidence {
                audio_end_ms: 1_200,
                classification: Target { score: 0.61 },
                stable_target: true,
            },
            LocalSpeakerEvidence {
                audio_end_ms: 2_400,
                classification: Uncertain { score: 0.40 },
                stable_target: true,
            },
            LocalSpeakerEvidence {
                audio_end_ms: 4_200,
                classification: NonTarget { score: 0.05 },
                stable_target: true,
            },
            LocalSpeakerEvidence {
                audio_end_ms: 5_000,
                classification: NonTarget { score: 0.12 },
                stable_target: false,
            },
            LocalSpeakerEvidence {
                audio_end_ms: 6_400,
                classification: Uncertain { score: 0.30 },
                stable_target: false,
            },
            LocalSpeakerEvidence {
                audio_end_ms: 7_800,
                classification: Uncertain { score: 0.38 },
                stable_target: false,
            },
            LocalSpeakerEvidence {
                audio_end_ms: 9_200,
                classification: Uncertain { score: 0.44 },
                stable_target: false,
            },
        ];
        let target_text = "开始录音你帮我看一下这个问题";
        let provider_result = json!({
            "text": "开始录音你帮我看一下这个问题我觉得归根结底是什么问题呢能不能修一下根因",
            "utterances": [
                {"text": target_text, "start_time": 100, "end_time": 4_400, "definite": true, "additions": {"speaker_id": "0", "source": "two_pass"}},
                {"text": "我觉得归根结底是什么问题呢能不能修一下根因", "start_time": 5_500, "end_time": 9_500, "definite": true, "additions": {"speaker_id": "1", "source": "two_pass"}}
            ]
        });
        assert!(
            sequential_speaker_split_gap_is_owner_safe(&state, &provider_result, target_text),
            "media-degraded owner split rows must survive the final gap gate by contrast"
        );
        // One confirmed non-owner window inside the split row must keep the
        // final gate closed: that is a second speaker, not a media dip.
        state.local_speaker_evidence.push(LocalSpeakerEvidence {
            audio_end_ms: 7_000,
            classification: NonTarget { score: 0.11 },
            stable_target: false,
        });
        assert!(
            !sequential_speaker_split_gap_is_owner_safe(&state, &provider_result, target_text),
            "a bystander window inside the split row must veto the final gate"
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
    fn same_cloud_speaker_id_keeps_later_utterance_when_identity_is_stable() {
        // Scores alone cannot distinguish this older bystander recording from
        // an owner's cross-phrase dip. Preserve the recognized main body while
        // the debounced identity still says owner.
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
        assert_eq!(filtered.result["text"], result["text"]);
        assert!(!filtered.stable_non_target_utterance_present);
    }

    #[test]
    fn owner_resuming_after_four_pauses_keeps_every_same_speaker_row() {
        // Installed session 350774d8: six stable cloud rows all had speaker 0.
        // The local owner identity never departed; some post-pause windows
        // scored 0.03 while later windows positively recognized the owner.
        // The old per-row low-score rule froze the ledger at 37 characters.
        let spans = [
            (280, 10_082, "开始录音。第一段正文。"),
            (12_662, 17_962, "停顿后第二段。"),
            (19_662, 24_091, "第三段。"),
            (25_922, 31_172, "第四段。"),
            (33_751, 46_362, "第五段还在继续说话。"),
            (50_282, 54_801, "最后一段不能吞。"),
        ];
        let utterances = spans
            .iter()
            .map(|(start, end, text)| {
                json!({
                    "additions": { "speaker_id": "0", "source": "two_pass" },
                    "definite": true,
                    "start_time": start,
                    "end_time": end,
                    "text": text,
                })
            })
            .collect::<Vec<_>>();
        let body = spans.iter().map(|(_, _, text)| *text).collect::<String>();
        let result = json!({ "text": body, "utterances": utterances });
        let evidence = [
            (2_000, 0.5000, true),
            (14_500, 0.4200, true),
            // The third provider row has no overlapping local sample.
            (28_000, 0.4100, true),
            (35_000, 0.4600, true),
            (38_000, 0.4069, true),
            (38_800, 0.1297, false),
            (39_600, 0.4046, true),
            (41_600, 0.4090, true),
            (48_800, 0.0310, false),
            (49_200, 0.0270, false),
            (51_200, 0.4300, true),
            (52_000, 0.5200, true),
        ]
        .into_iter()
        .map(|(audio_end_ms, score, target)| LocalSpeakerEvidence {
            audio_end_ms,
            classification: if target {
                crate::speaker_verification::SessionSpeakerClassification::Target { score }
            } else {
                crate::speaker_verification::SessionSpeakerClassification::Uncertain { score }
            },
            stable_target: true,
        })
        .collect::<Vec<_>>();
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
            Some(49_200),
            None,
        );
        assert_eq!(target.as_deref(), Some("0"));
        assert_eq!(filtered.result["text"], result["text"]);
        assert!(!filtered.stable_non_target_utterance_present);
    }

    #[test]
    fn long_phrase_only_wake_keeps_early_rows_past_former_64_window_limit() {
        // Installed session 3a70d839: wake phrase passed but voiceprint was
        // inconclusive. At final time a 64-window ring no longer overlapped
        // the early cloud rows, although all six stable rows belonged to
        // speaker 0. Preserve the entire session's time-aligned evidence.
        let spans = [
            (280, 10_342, "开始录音。第一段正文。"),
            (11_252, 15_722, "第一次停顿后继续。"),
            (17_372, 22_261, "第二段继续。"),
            (22_261, 27_851, "第二次停顿后继续。"),
            (28_922, 33_501, "第三次停顿后继续。"),
            (36_051, 43_242, "最后一句也要保留。"),
        ];
        let utterances = spans
            .iter()
            .map(|(start, end, text)| {
                json!({
                    "additions": { "speaker_id": "0", "source": "two_pass" },
                    "definite": true,
                    "start_time": start,
                    "end_time": end,
                    "text": text,
                })
            })
            .collect::<Vec<_>>();
        let body = spans.iter().map(|(_, _, text)| *text).collect::<String>();
        let result = json!({ "text": body, "utterances": utterances });
        let evidence = (0..109)
            .map(|index| LocalSpeakerEvidence {
                audio_end_ms: 1_200 + 400 * index as u64,
                classification:
                    crate::speaker_verification::SessionSpeakerClassification::Uncertain {
                        score: 0.28,
                    },
                stable_target: true,
            })
            .collect::<Vec<_>>();
        let mut target = None;
        let filtered = filter_result_to_target_speaker_with_local_evidence_and_anchor(
            &result,
            &mut target,
            true,
            &evidence,
            Some("开始录音"),
            None,
            false,
            false,
            None,
            None,
        );
        assert_eq!(target.as_deref(), Some("0"));
        assert_eq!(filtered.result["text"], result["text"]);
        assert!(!filtered.stable_other_speaker_present);
        assert!(!filtered.stable_non_target_utterance_present);
    }

    #[test]
    fn phrase_only_wake_later_target_match_refreshes_owner_endpoint() {
        use crate::speaker_verification::SessionSpeakerClassification::Uncertain;
        // Session 3a70d839: wake passed by phrase, body later matched the
        // enrolled owner, and a resumed clause kept growing after the older
        // qualified window. The wake flag must not permanently bar that body
        // from refreshing the owner endpoint.
        let mut state = SyncState {
            local_speaker_tracking_enabled: true,
            local_wake_owner_verified: false,
            local_speaker_stable_target: true,
            local_target_confirmed: true,
            local_speaker_classification: Some(Uncertain { score: 0.31 }),
            local_audio_duration_ms: Some(43_100),
            local_target_speech_end_ms: Some(37_000),
            qualified_owner_speech_end_ms: Some(23_400),
            local_sustained_non_target_speech_end_ms: Some(18_200),
            ..SyncState::default()
        };
        assert_eq!(local_owner_continuity(&state), LocalOwnerContinuity::Compatible);
        assert!(refresh_local_target_from_owner_preview_activity(&mut state));
        assert_eq!(state.local_target_speech_end_ms, Some(43_100));
    }

    #[test]
    fn verified_wake_does_not_delete_same_cloud_body_on_low_scores_alone() {
        // A verified wake anchors the owner, but low scores in later phrases
        // cannot independently prove that another person took over.
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
            None,
        );
        assert_eq!(target.as_deref(), Some("0"));
        assert_eq!(filtered.result["text"], result["text"]);
        assert!(!filtered.stable_non_target_utterance_present);

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
            None,
        );
        assert_eq!(
            adaptive.result["text"], result["text"],
            "a wake-derived adaptive profile must remain non-destructive"
        );
        assert!(!adaptive.stable_non_target_utterance_present);
    }

    #[test]
    fn anchor_abstains_when_tracking_lost_and_no_identity_source() {
        // r17 field regression (work/human-endpoint-cd-r17-20260917): the live
        // wake accept ran but local tracking never activated, so the filter
        // anchored the target onto the first stable utterance — a bystander's
        // leading sentence — and the kernel veto then destroyed the owner's
        // whole body. Without an identity source the filter must abstain:
        // no anonymous first-speaker anchor, no non-target veto, and raw
        // recovery stays available so the owner text cannot be swallowed.
        let result = json!({
            "text": "旁人先开口说话。主人正文第一部分必须完整保留。",
            "utterances": [
                {
                    "additions": { "speaker_id": "0", "source": "two_pass" },
                    "definite": true,
                    "start_time": 0,
                    "end_time": 3_400,
                    "text": "旁人先开口说话。"
                },
                {
                    "additions": { "speaker_id": "1", "source": "two_pass" },
                    "definite": true,
                    "start_time": 4_200,
                    "end_time": 12_800,
                    "text": "主人正文第一部分必须完整保留。"
                }
            ]
        });
        let mut target = None;
        let filtered = filter_result_to_target_speaker_with_local_evidence_and_anchor(
            &result,
            &mut target,
            false,
            &[],
            None,
            None,
            false,
            false,
            None,
            None,
        );
        assert!(
            target.is_none(),
            "no identity source may anchor to an anonymous first speaker"
        );
        assert_eq!(
            filtered.result["text"], "",
            "the abstaining filter must not select any utterance"
        );
        assert!(
            !filtered.stable_non_target_utterance_present,
            "without a target there is no non-target veto material"
        );
        assert_eq!(
            crate::speech_decision_kernel::arbitrate_final_transcript(
                crate::speech_decision_kernel::FinalTranscriptEvidence {
                    protocol_final: true,
                    explicit_non_owner_tail: filtered.stable_non_target_utterance_present,
                    provider_raw_recovery_safe: true,
                    ..Default::default()
                }
            ),
            crate::speech_decision_kernel::FinalTranscriptAuthority::ProviderRawRecovery,
            "raw recovery must stay available so tracking loss cannot swallow text (E)"
        );
    }

    #[test]
    fn wake_phrase_row_binds_target_even_when_tracking_flag_lost() {
        // The wake phrase is session identity independent of the runtime
        // tracking flag: the cloud row that spoke the phrase is definitionally
        // the wake speaker, so it stays a valid anchor even when tracking
        // activation was lost.
        let result = json!({
            "text": "旁人先开口说话。开始录音。主人正文。",
            "utterances": [
                {
                    "additions": { "speaker_id": "0", "source": "two_pass" },
                    "definite": true,
                    "start_time": 0,
                    "end_time": 3_400,
                    "text": "旁人先开口说话。"
                },
                {
                    "additions": { "speaker_id": "1", "source": "two_pass" },
                    "definite": true,
                    "start_time": 3_800,
                    "end_time": 6_200,
                    "text": "开始录音。"
                },
                {
                    "additions": { "speaker_id": "1", "source": "two_pass" },
                    "definite": true,
                    "start_time": 6_400,
                    "end_time": 11_000,
                    "text": "主人正文。"
                }
            ]
        });
        let mut target = None;
        let filtered = filter_result_to_target_speaker_with_local_evidence_and_anchor(
            &result,
            &mut target,
            false,
            &[],
            Some("开始录音"),
            None,
            false,
            false,
            None,
            None,
        );
        assert_eq!(
            target.as_deref(),
            Some("1"),
            "the cloud row that spoke the wake phrase anchors the target"
        );
        assert!(
            !filtered.result["text"]
                .as_str()
                .unwrap_or_default()
                .contains("旁人先开口说话。"),
            "the bystander's leading sentence must not enter the filtered result"
        );
    }

    #[test]
    fn local_evidence_never_binds_without_tracking_flag() {
        let result = json!({
            "text": "旁人先开口说话。主人正文第一部分必须完整保留。",
            "utterances": [
                {
                    "additions": { "speaker_id": "0", "source": "two_pass" },
                    "definite": true,
                    "start_time": 0,
                    "end_time": 3_400,
                    "text": "旁人先开口说话。"
                },
                {
                    "additions": { "speaker_id": "1", "source": "two_pass" },
                    "definite": true,
                    "start_time": 4_200,
                    "end_time": 12_800,
                    "text": "主人正文第一部分必须完整保留。"
                }
            ]
        });
        let evidence = vec![LocalSpeakerEvidence {
            audio_end_ms: 9_000,
            classification: crate::speaker_verification::SessionSpeakerClassification::Target {
                score: 0.72,
            },
            stable_target: true,
        }];
        let mut target = None;
        filter_result_to_target_speaker_with_local_evidence_and_anchor(
            &result,
            &mut target,
            false,
            &evidence,
            None,
            None,
            false,
            false,
            None,
            None,
        );
        assert!(
            target.is_none(),
            "local evidence must not bind a target while tracking is disabled"
        );

        let mut tracked_target = None;
        filter_result_to_target_speaker_with_local_evidence_and_anchor(
            &result,
            &mut tracked_target,
            true,
            &evidence,
            None,
            None,
            false,
            false,
            None,
            None,
        );
        assert_eq!(
            tracked_target.as_deref(),
            Some("1"),
            "with tracking active the local owner evidence binds the target"
        );
    }

    #[test]
    fn r17_shape_bystander_prefix_keeps_owner_body_via_local_evidence() {
        // The r17 scenario with a working anchor: a bystander speaks first,
        // the wake phrase row is not visible to the provider result, and the
        // local session-speaker evidence (owner-enrolled bank) classifies the
        // bystander prefix NonTarget and the owner body Target. The filtered
        // result must keep the owner's full body text and exclude the
        // bystander, and the kernel must deliver that text.
        let result = json!({
            "text": "是一百分，我看一下这个。我现在做端点复测，第一部分先完整保留，最后这句话也必须完整保留。",
            "utterances": [
                {
                    "additions": { "speaker_id": "0", "source": "two_pass" },
                    "definite": true,
                    "start_time": 0,
                    "end_time": 3_400,
                    "text": "是一百分，我看一下这个。"
                },
                {
                    "additions": { "speaker_id": "1", "source": "two_pass" },
                    "definite": true,
                    "start_time": 4_200,
                    "end_time": 16_800,
                    "text": "我现在做端点复测，第一部分先完整保留，最后这句话也必须完整保留。"
                }
            ]
        });
        let evidence = vec![
            LocalSpeakerEvidence {
                audio_end_ms: 2_400,
                classification:
                    crate::speaker_verification::SessionSpeakerClassification::NonTarget {
                        score: 0.08,
                    },
                stable_target: false,
            },
            LocalSpeakerEvidence {
                audio_end_ms: 3_200,
                classification:
                    crate::speaker_verification::SessionSpeakerClassification::NonTarget {
                        score: 0.06,
                    },
                stable_target: false,
            },
            LocalSpeakerEvidence {
                audio_end_ms: 6_000,
                classification:
                    crate::speaker_verification::SessionSpeakerClassification::Target {
                        score: 0.72,
                    },
                stable_target: true,
            },
            LocalSpeakerEvidence {
                audio_end_ms: 9_600,
                classification:
                    crate::speaker_verification::SessionSpeakerClassification::Target {
                        score: 0.68,
                    },
                stable_target: true,
            },
            LocalSpeakerEvidence {
                audio_end_ms: 13_200,
                classification:
                    crate::speaker_verification::SessionSpeakerClassification::Target {
                        score: 0.70,
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
            false,
            false,
            None,
            None,
        );
        assert_eq!(
            target.as_deref(),
            Some("1"),
            "the owner body cluster must win the anchor through local evidence"
        );
        let filtered_text = filtered.result["text"].as_str().unwrap_or_default();
        assert!(
            filtered_text.contains("我现在做端点复测，第一部分先完整保留"),
            "the owner's body must survive the filter, got: {filtered_text}"
        );
        assert!(
            !filtered_text.contains("是一百分"),
            "the bystander's text must not enter the filtered result"
        );
        assert!(
            filtered.stable_other_speaker_present,
            "the bystander row must be visible as a foreign speaker so \
             final_explicit_non_owner_tail can veto raw recovery"
        );
        assert_eq!(
            crate::speech_decision_kernel::arbitrate_final_transcript(
                crate::speech_decision_kernel::FinalTranscriptEvidence {
                    protocol_final: true,
                    explicit_non_owner_tail: true,
                    ..Default::default()
                }
            ),
            crate::speech_decision_kernel::FinalTranscriptAuthority::SpeakerFiltered,
            "with a correct owner anchor the kernel delivers the filtered owner text"
        );
    }

    #[test]
    fn reanchor_local_speaker_tracking_heals_lost_activation() {
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        assert!(
            !asr.local_speaker_tracking_active(),
            "a fresh instance must start unanchored"
        );
        assert!(
            asr.reanchor_local_speaker_tracking_if_lost("开始录音", false),
            "the first heal must report that it re-anchored"
        );
        assert!(asr.local_speaker_tracking_active());
        {
            let state = asr.state.lock();
            assert_eq!(state.wake_speaker_phrase.as_deref(), Some("开始录音"));
        }
        assert!(
            !asr.reanchor_local_speaker_tracking_if_lost("开始录音", false),
            "a healthy anchor must not be healed again"
        );
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
    fn protocol_final_preserves_same_cluster_body_without_identity_departure() {
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
        assert_eq!(transcript.text, "开始录音。主人正文。旁人的话不能放进去。");
        assert!(!asr.state.lock().owner_isolation_frozen);
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
                target_speaker_update_from_state(&state, false, false, false)
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
                target_speaker_update_from_state(&state, false, false, false).local_non_target_speech_end_ms,
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
    fn target_speaker_endpoint_keeps_established_owner_through_uncertain_cross_phrase_window() {
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        asr.note_local_speaker_tracking_started_with_owner("开始录音", true);
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
        // The sample itself is not activity. A later owner-safe preview
        // revision uses this retained continuity verdict to advance the clock.
        assert_eq!(state.local_target_speech_end_ms, Some(1_800));
        assert_eq!(
            local_owner_continuity(&state),
            LocalOwnerContinuity::Compatible
        );
    }

    #[test]
    fn owner_preview_growth_refreshes_local_target_while_stable() {
        let mut state = SyncState {
            local_speaker_tracking_enabled: true,
            local_wake_owner_verified: true,
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
    fn target_speaker_endpoint_and_preview_share_uncertain_owner_continuity() {
        // Installed session 417 exposed the inverse split: the preview admitted
        // a debounced Uncertain owner revision but endpointing called it Quiet
        // and stopped before the provider attributed the same words. Both
        // consumers now read one continuity verdict.
        let mut state = SyncState {
            local_speaker_tracking_enabled: true,
            local_wake_owner_verified: true,
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

        assert!(local_speaker_allows_optimistic_preview(&state));
        assert!(refresh_local_target_from_owner_safe_provider_split(
            &mut state
        ));
        assert_eq!(state.local_target_speech_end_ms, Some(8_940));

        state.local_audio_duration_ms = Some(9_340);
        state.local_speaker_classification = Some(
            crate::speaker_verification::SessionSpeakerClassification::NonTarget { score: 0.10 },
        );
        assert!(!local_speaker_allows_optimistic_preview(&state));
        assert!(!refresh_local_target_from_owner_safe_provider_split(
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
            None,
        );
        assert_eq!(rejected.result["text"], "");
    }

    #[test]
    fn installed_session_353_keeps_verified_owner_after_full_final_cluster_split() {
        // Installed session 353 returned the physical wake as speaker 0 and
        // split the uninterrupted owner body into two stable speaker-1 rows.
        // The first body row began only 360 ms after the wake ended, while the
        // local verifier retained the owner for the complete body. Pinning the
        // provider id to speaker 0 reduced the final to the wake phrase and the
        // subsequent wake-prefix removal produced an empty transcript.
        let wake = "开始录音。";
        let body_first = "我现在说的这一整段正文不能因为云端换了编号就被删除。";
        let body_last = "停顿以后最后这一句话也必须完整保留。";
        let result = json!({
            "text": format!("{wake}{body_first}{body_last}"),
            "utterances": [
                {
                    "additions": { "speaker_id": "0", "source": "two_pass" },
                    "definite": true,
                    "start_time": 142,
                    "end_time": 1_022,
                    "text": wake
                },
                {
                    "additions": { "speaker_id": "1", "source": "two_pass" },
                    "definite": true,
                    "start_time": 1_382,
                    "end_time": 6_502,
                    "text": body_first
                },
                {
                    "additions": { "speaker_id": "1", "source": "two_pass" },
                    "definite": true,
                    "start_time": 6_502,
                    "end_time": 10_732,
                    "text": body_last
                }
            ]
        });
        let evidence = vec![
            LocalSpeakerEvidence {
                audio_end_ms: 1_300,
                classification: crate::speaker_verification::SessionSpeakerClassification::Target {
                    score: 0.73,
                },
                stable_target: true,
            },
            LocalSpeakerEvidence {
                audio_end_ms: 2_300,
                classification:
                    crate::speaker_verification::SessionSpeakerClassification::Uncertain {
                        score: 0.42,
                    },
                stable_target: true,
            },
            LocalSpeakerEvidence {
                audio_end_ms: 4_200,
                classification:
                    crate::speaker_verification::SessionSpeakerClassification::Uncertain {
                        score: 0.38,
                    },
                stable_target: true,
            },
            LocalSpeakerEvidence {
                audio_end_ms: 6_100,
                classification:
                    crate::speaker_verification::SessionSpeakerClassification::Uncertain {
                        score: 0.41,
                    },
                stable_target: true,
            },
            LocalSpeakerEvidence {
                audio_end_ms: 7_600,
                classification:
                    crate::speaker_verification::SessionSpeakerClassification::Uncertain {
                        score: 0.36,
                    },
                stable_target: true,
            },
            LocalSpeakerEvidence {
                audio_end_ms: 10_200,
                classification:
                    crate::speaker_verification::SessionSpeakerClassification::Uncertain {
                        score: 0.40,
                    },
                stable_target: true,
            },
        ];
        let mut target = Some("0".to_string());

        let filtered = filter_result_to_target_speaker_with_local_evidence_and_anchor(
            &result,
            &mut target,
            true,
            &evidence,
            Some("开始录音"),
            Some(1_022),
            true,
            false,
            None,
            None,
        );

        assert_eq!(target.as_deref(), Some("0"), "wake anchor stays persistent");
        assert_eq!(
            filtered.result["text"],
            format!("{wake}{body_first}{body_last}")
        );
        assert_eq!(filtered.target_speech_end_ms, Some(10_732));
        assert!(!filtered.stable_other_speaker_present);
        assert!(filtered.response_local_body_alias_present);

        #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
        {
            let separated_wake_only = RawTranscript {
                text: wake.to_string(),
                duration_ms: 10_732,
            };
            let provider_owner_track = RawTranscript {
                text: filtered.result["text"]
                    .as_str()
                    .expect("filtered owner text")
                    .to_string(),
                duration_ms: 10_732,
            };
            let recovered = recover_incomplete_separator_final_from_distinct_provider_track(
                Ok(Some(separated_wake_only)),
                Some(provider_owner_track),
            )
            .expect("provider owner track recovery succeeds")
            .expect("provider owner track remains available");
            assert_eq!(recovered.text, format!("{wake}{body_first}{body_last}"));
        }
    }

    #[test]
    fn full_final_cluster_alias_requires_tight_boundary_and_retained_owner() {
        let make_result = |body_start_ms| {
            json!({
                "text": "开始录音。旁人后来才说话。",
                "utterances": [
                    {
                        "additions": { "speaker_id": "0", "source": "two_pass" },
                        "definite": true,
                        "start_time": 100,
                        "end_time": 1_000,
                        "text": "开始录音。"
                    },
                    {
                        "additions": { "speaker_id": "1", "source": "two_pass" },
                        "definite": true,
                        "start_time": body_start_ms,
                        "end_time": 4_000,
                        "text": "旁人后来才说话。"
                    }
                ]
            })
        };
        let retained_owner = vec![LocalSpeakerEvidence {
            audio_end_ms: 2_600,
            classification: crate::speaker_verification::SessionSpeakerClassification::Uncertain {
                score: 0.42,
            },
            stable_target: true,
        }];
        let mut target = Some("0".to_string());
        let late = filter_result_to_target_speaker_with_local_evidence_and_anchor(
            &make_result(1_700),
            &mut target,
            true,
            &retained_owner,
            Some("开始录音"),
            Some(1_000),
            true,
            false,
            None,
            None,
        );
        assert_eq!(late.result["text"], "开始录音。");

        let departed_owner = vec![LocalSpeakerEvidence {
            audio_end_ms: 2_000,
            classification: crate::speaker_verification::SessionSpeakerClassification::NonTarget {
                score: 0.12,
            },
            stable_target: false,
        }];
        let contiguous = filter_result_to_target_speaker_with_local_evidence_and_anchor(
            &make_result(1_200),
            &mut target,
            true,
            &departed_owner,
            Some("开始录音"),
            Some(1_000),
            true,
            false,
            Some(2_000),
            None,
        );
        assert_eq!(contiguous.result["text"], "开始录音。");
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
    fn target_speaker_endpoint_installed_session_621_keeps_one_owner_split_across_three_cloud_ids()
    {
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
    fn target_speaker_endpoint_installed_session_2744_does_not_restore_confirmed_foreign_tail() {
        // Live airborne interference, 2026-09-04: the provider correctly
        // separated the 44-char owner row from a 7-char speaker-1 tail. Two
        // consecutive transcript-grade local mismatches overlapped speaker 1,
        // but the advisory classification timeline still carried the latched
        // stable_target bit. The old cloud-cluster-drift recovery therefore
        // restored all 51 chars and leaked “会持续一段时”. Confirmed hard local
        // evidence must veto only that fail-open expansion, without deleting
        // the already selected owner row.
        let owner = "开始录音。主人第一句，我在旁边说话时自然停顿一下。现在说主人第二句，最后这句话也不能丢。";
        let result = json!({
            "text": format!("{owner}会持续一段时。"),
            "utterances": [
                {
                    "additions": { "speaker_id": "0", "source": "two_pass" },
                    "definite": true,
                    "start_time": 1_072,
                    "end_time": 8_192,
                    "text": owner
                },
                {
                    "additions": { "speaker_id": "1", "source": "two_pass" },
                    "definite": true,
                    "start_time": 8_272,
                    "end_time": 9_702,
                    "text": "会持续一段时。"
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
            local_speaker_evidence: vec![
                LocalSpeakerEvidence {
                    audio_end_ms: 7_000,
                    classification:
                        crate::speaker_verification::SessionSpeakerClassification::Target {
                            score: 0.69,
                        },
                    stable_target: true,
                },
                LocalSpeakerEvidence {
                    audio_end_ms: 9_000,
                    classification:
                        crate::speaker_verification::SessionSpeakerClassification::Uncertain {
                            score: 0.051,
                        },
                    stable_target: true,
                },
                LocalSpeakerEvidence {
                    audio_end_ms: 9_400,
                    classification:
                        crate::speaker_verification::SessionSpeakerClassification::Uncertain {
                            score: 0.046,
                        },
                    stable_target: true,
                },
                LocalSpeakerEvidence {
                    audio_end_ms: 9_800,
                    classification:
                        crate::speaker_verification::SessionSpeakerClassification::Uncertain {
                            score: 0.108,
                        },
                    stable_target: true,
                },
            ],
            ..SyncState::default()
        };

        assert!(
            !sequential_speaker_split_gap_is_owner_safe(&state, &result, owner),
            "a latched stable-target state cannot recover a later row whose interval has no fresh owner support"
        );
        state.local_confirmed_transcript_non_target_end_ms = Some(9_400);
        assert!(!sequential_speaker_split_gap_is_owner_safe(
            &state, &result, owner,
        ));
        assert!(stable_provider_foreign_row_has_local_veto(&state, &result));

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
        let payload = serde_json::to_vec(&json!({
            "audio_info": { "duration": 9_700 },
            "result": result,
        }))
        .expect("session 2744 final serializes");
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
            .expect("session 2744 final should resolve")
            .expect("confirmed owner row should remain successful");
        assert_eq!(transcript.text, owner);
    }

    #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
    #[test]
    fn provider_final_can_close_a_confirmed_late_non_target_tail_without_separator_wait() {
        assert!(provider_final_excludes_late_non_target_tail(
            true,
            Some(15_322),
            Some(17_100),
        ));
        assert!(!provider_final_excludes_late_non_target_tail(
            false,
            Some(15_322),
            Some(17_100),
        ));
        assert!(!provider_final_excludes_late_non_target_tail(
            true,
            Some(16_700),
            Some(17_100),
        ));
        assert!(!provider_final_excludes_late_non_target_tail(
            true,
            None,
            Some(17_100),
        ));
    }

    #[test]
    fn open_owner_row_after_short_foreign_row_can_stream_with_fresh_voiceprint_evidence() {
        let owner = "开始录音。已确认第一段。";
        let foreign = "旁人一句";
        let continuation = "继续说自己的正文";
        let result = json!({
            "text": format!("{owner}{foreign}{continuation}"),
            "utterances": [
                {"additions": {"speaker_id": "0"}, "definite": true,
                 "start_time": 0, "end_time": 5_000, "text": owner},
                {"additions": {"speaker_id": "1"}, "definite": true,
                 "start_time": 5_200, "end_time": 6_000, "text": foreign},
                {"additions": {"speaker_id": "0"}, "definite": false,
                 "start_time": 6_100, "text": continuation}
            ]
        });
        let sample = |audio_end_ms, classification| LocalSpeakerEvidence {
            audio_end_ms,
            classification,
            stable_target: true,
        };
        let target = |score| crate::speaker_verification::SessionSpeakerClassification::Target { score };
        let uncertain = |score| crate::speaker_verification::SessionSpeakerClassification::Uncertain { score };
        let mut evidence = vec![
            sample(1_800, target(0.48)),
            sample(2_200, target(0.49)),
            sample(6_400, uncertain(0.33)),
            sample(7_400, target(0.44)),
            sample(7_800, target(0.46)),
        ];
        let filter = |evidence: &[LocalSpeakerEvidence]| {
            filter_result_to_target_speaker_with_local_evidence_and_anchor(
                &result,
                &mut Some("0".into()),
                true,
                evidence,
                Some("开始录音"),
                Some(1_000),
                false,
                false,
                None,
                None,
            )
        };
        let admitted = filter(&evidence);
        assert_eq!(admitted.result["text"], owner);
        assert_eq!(admitted.optimistic_result["text"], format!("{owner}{continuation}"));
        assert!(!admitted.optimistic_result["text"].as_str().unwrap().contains(foreign));

        evidence.pop();
        assert_eq!(filter(&evidence).optimistic_result["text"], owner);
        evidence.push(sample(7_800, target(0.46)));
        evidence.push(sample(
            8_200,
            crate::speaker_verification::SessionSpeakerClassification::NonTarget { score: 0.10 },
        ));
        assert_eq!(filter(&evidence).optimistic_result["text"], owner);
    }

    #[test]
    fn final_foreign_row_without_fresh_owner_support_cannot_reopen_raw_provider_text() {
        // Installed session 821bf512: speaker 0 finished the owner body at
        // 5702 ms and speaker 1 started a separate tail at 6422 ms. Every
        // local window aligned to the later row stayed at or below the 0.30
        // owner-absence boundary, while the historical stable-target latch
        // remained true. The old split recovery therefore restored speaker 1.
        let owner = "开始录音。帮我看一下，现在感觉好像是越来越差了。";
        let foreign = "旁边的人正在讨论今晚吃什么，这些话不";
        let result = json!({
            "text": format!("{owner}{foreign}"),
            "utterances": [
                {
                    "additions": { "speaker_id": "0", "source": "two_pass" },
                    "definite": true,
                    "start_time": 120,
                    "end_time": 2041,
                    "text": "开始录音。"
                },
                {
                    "additions": { "speaker_id": "0", "source": "two_pass" },
                    "definite": true,
                    "start_time": 3702,
                    "end_time": 5702,
                    "text": "帮我看一下，现在感觉好像是越来越差了。"
                },
                {
                    "additions": { "speaker_id": "1", "source": "two_pass" },
                    "definite": true,
                    "start_time": 6422,
                    "end_time": 10242,
                    "text": foreign
                }
            ]
        });
        let state = SyncState {
            local_speaker_tracking_enabled: true,
            local_speaker_profile_adaptive: false,
            local_speaker_stable_target: true,
            local_target_confirmed: true,
            target_speaker_id: Some("0".into()),
            wake_speaker_phrase: Some("开始录音".into()),
            local_speaker_evidence: [
                (7100, 0.161),
                (7500, 0.219),
                (7900, 0.220),
                (8300, 0.208),
                (8700, 0.296),
                (9100, 0.252),
                (9500, 0.251),
                (9900, 0.255),
            ]
            .into_iter()
            .map(|(audio_end_ms, score)| LocalSpeakerEvidence {
                audio_end_ms,
                classification:
                    crate::speaker_verification::SessionSpeakerClassification::Uncertain { score },
                stable_target: true,
            })
            .collect(),
            ..SyncState::default()
        };
        let filtered = SpeakerFilteredResult {
            result: json!({ "text": owner }),
            optimistic_result: json!({ "text": owner }),
            speaker_info_present: true,
            response_local_body_alias_present: false,
            stable_non_target_utterance_present: false,
            stable_other_speaker_present: true,
            stable_unresolved_speaker_present: false,
            target_speech_end_ms: Some(5702),
            wake_target_speech_end_ms: Some(2041),
            stable_attributed_speech_end_ms: Some(10242),
            pending_unattributed_text: String::new(),
        };

        assert!(!sequential_speaker_split_gap_is_owner_safe(
            &state, &result, owner,
        ));
        assert!(final_explicit_non_owner_tail(&state, &filtered, &result));
        assert_eq!(
            crate::speech_decision_kernel::arbitrate_final_transcript(
                crate::speech_decision_kernel::FinalTranscriptEvidence {
                    protocol_final: true,
                    explicit_non_owner_tail: true,
                    provider_raw_recovery_safe: false,
                    provider_owner_recovery_safe: true,
                    session_ledger_recovery_safe: true,
                    optimistic_owner_recovery_safe: true,
                },
            ),
            crate::speech_decision_kernel::FinalTranscriptAuthority::SpeakerFiltered
        );
    }

    #[test]
    fn same_cloud_owner_row_awaiting_local_coverage_is_not_foreign() {
        // Installed 4ec1bb64: the provider sealed three stable speaker-0 rows.
        // The last eight characters occupied 21_441..22_181 ms, while the
        // local verifier's final two window centres were only 20_800/21_200.
        // A missing vote in that unobserved 241 ms gap discarded the tail and
        // falsely marked it as another speaker.
        let rows = json!([
            {"additions":{"speaker_id":"0","source":"two_pass"},"definite":true,"start_time":240,"end_time":1_680,"text":"开始录音"},
            {"additions":{"speaker_id":"0","source":"two_pass"},"definite":true,"start_time":1_680,"end_time":21_441,"text":"那就不对啊，你看一下。"},
            {"additions":{"speaker_id":"0","source":"two_pass"},"definite":true,"start_time":21_441,"end_time":22_181,"text":"然后我看怎么样。"}
        ]);
        let provider = json!({
            "text":"开始录音那就不对啊，你看一下。然后我看怎么样。",
            "utterances":rows
        });
        let evidence = [(18_000, 0.51), (19_000, 0.49), (21_400, 0.28), (21_800, 0.202)]
            .into_iter()
            .map(|(audio_end_ms, score)| LocalSpeakerEvidence {
                audio_end_ms,
                classification: if score >= 0.40 {
                    crate::speaker_verification::SessionSpeakerClassification::Target { score }
                } else {
                    crate::speaker_verification::SessionSpeakerClassification::Uncertain { score }
                },
                stable_target: true,
            })
            .collect::<Vec<_>>();
        let mut target = Some("0".to_string());
        let filtered = filter_result_to_target_speaker_with_local_evidence_and_anchor(
            &provider, &mut target, true, &evidence, Some("开始录音"), Some(1_680),
            false, false, Some(20_600), None,
        );
        assert_eq!(filtered.result["text"], provider["text"]);
        assert!(!filtered.stable_other_speaker_present);

        let mut foreign_evidence = evidence.clone();
        foreign_evidence.last_mut().unwrap().classification =
            crate::speaker_verification::SessionSpeakerClassification::NonTarget { score: 0.15 };
        let rejected = filter_result_to_target_speaker_with_local_evidence_and_anchor(
            &provider, &mut target, true, &foreign_evidence, Some("开始录音"), Some(1_680),
            false, false, Some(21_800), None,
        );
        assert!(!rejected.result["text"].as_str().unwrap().contains("然后我看怎么样"));

        let mut foreign_provider = provider.clone();
        foreign_provider["utterances"][2]["additions"]["speaker_id"] = json!("1");
        let foreign_row = filter_result_to_target_speaker_with_local_evidence_and_anchor(
            &foreign_provider, &mut target, true, &evidence, Some("开始录音"), Some(1_680),
            false, false, Some(20_600), None,
        );
        assert!(foreign_row.stable_other_speaker_present);
        assert!(!foreign_row.result["text"].as_str().unwrap().contains("然后我看怎么样"));
    }

    #[test]
    fn open_wake_uncertain_owner_uses_one_preview_and_endpoint_verdict() {
        use crate::speaker_verification::SessionSpeakerClassification::{NonTarget, Uncertain};

        // Installed 4ec1bb64: the bank failed the wake, later body windows
        // established the owner, and low-score Uncertain windows kept the
        // absence run latched while the same cloud speaker kept talking.
        let mut state = SyncState {
            local_speaker_tracking_enabled: true,
            local_wake_owner_verified: false,
            local_speaker_stable_target: true,
            local_target_confirmed: true,
            local_owner_absence_run_confirmed: true,
            local_sustained_non_target_speech_end_ms: Some(21_800),
            qualified_owner_speech_end_ms: Some(7_000),
            local_target_speech_end_ms: Some(19_000),
            local_audio_duration_ms: Some(21_800),
            local_speech_end_ms: Some(21_800),
            local_speaker_classification: Some(Uncertain { score: 0.202 }),
            target_speaker_id: Some("0".into()),
            ..SyncState::default()
        };
        assert_eq!(local_owner_continuity(&state), LocalOwnerContinuity::Compatible);
        assert_eq!(
            target_speaker_update_from_state(&state, false, false, false)
                .local_non_target_speech_end_ms,
            None,
        );
        assert!(refresh_local_target_from_owner_preview_activity(&mut state));
        assert_eq!(state.local_target_speech_end_ms, Some(21_800));

        state.local_speaker_classification = Some(NonTarget { score: 0.08 });
        assert_eq!(local_owner_continuity(&state), LocalOwnerContinuity::Other);
        assert_eq!(
            target_speaker_update_from_state(&state, false, false, false)
                .local_non_target_speech_end_ms,
            Some(21_800),
        );
        state.local_speaker_classification = Some(Uncertain { score: 0.202 });
        state.local_wake_owner_verified = true;
        assert_eq!(local_owner_continuity(&state), LocalOwnerContinuity::Other);
    }

    #[test]
    fn unverified_wake_cluster_split_body_is_session_owned_not_foreign() {
        // Live sessions 0dbc59da / 86768c0e (2026-09-20): the enrolled bank
        // drifted to a non-match, the wake was accepted through open
        // acceptance (local_wake_owner_verified=false), and cloud diarization
        // split the user's own continuous body into a stable speaker-1
        // cluster while every local window read drifted-owner Uncertain
        // (0.16-0.30, above the 0.10 hard-NonTarget floor). The filter kept
        // only "开始录音。", the wake-phrase strip emptied the delivery, and
        // the product reported "没有识别到语音" for 44 chars it had previewed.
        let body = "这个任务是为什么要关闭呢？是另外的一个并行的任务。就是你这个任务，一瞬间只能执行一个吗？";
        let result = json!({
            "text": format!("开始录音。{body}"),
            "utterances": [
                {
                    "additions": { "speaker_id": "0", "source": "two_pass" },
                    "definite": true,
                    "start_time": 1032,
                    "end_time": 2018,
                    "text": "开始录音。"
                },
                {
                    "additions": { "speaker_id": "1", "source": "two_pass" },
                    "definite": true,
                    "start_time": 2653,
                    "end_time": 10977,
                    "text": body,
                }
            ]
        });
        let base_state = || SyncState {
            local_speaker_tracking_enabled: true,
            local_wake_owner_verified: false,
            local_speaker_profile_adaptive: false,
            local_speaker_stable_target: true,
            target_speaker_id: Some("0".into()),
            wake_speaker_phrase: Some("开始录音".into()),
            ..SyncState::default()
        };
        let filtered = SpeakerFilteredResult {
            result: json!({ "text": "开始录音。" }),
            optimistic_result: json!({ "text": "开始录音。" }),
            speaker_info_present: true,
            response_local_body_alias_present: false,
            stable_non_target_utterance_present: true,
            stable_other_speaker_present: true,
            stable_unresolved_speaker_present: false,
            target_speech_end_ms: Some(2018),
            wake_target_speech_end_ms: Some(2018),
            stable_attributed_speech_end_ms: Some(10977),
            pending_unattributed_text: String::new(),
        };

        assert!(open_session_wake_owned_body_recovery(
            &base_state(),
            &filtered,
            &result
        ));
        assert!(!final_explicit_non_owner_tail(
            &base_state(),
            &filtered,
            &result
        ));

        // A bank-verified wake licenses the strict isolation: the same
        // cluster split stays an explicit non-owner veto.
        let verified_wake_state = SyncState {
            local_wake_owner_verified: true,
            ..base_state()
        };
        assert!(!open_session_wake_owned_body_recovery(
            &verified_wake_state,
            &filtered,
            &result
        ));
        assert!(final_explicit_non_owner_tail(
            &verified_wake_state,
            &filtered,
            &result
        ));

        // A latched hard NonTarget window (media / a real second speaker)
        // keeps the absolute veto even when the wake itself was unverified.
        let hard_non_target_state = SyncState {
            local_non_target_speech_end_ms: Some(10_500),
            ..base_state()
        };
        assert!(!open_session_wake_owned_body_recovery(
            &hard_non_target_state,
            &filtered,
            &result
        ));
        assert!(final_explicit_non_owner_tail(
            &hard_non_target_state,
            &filtered,
            &result
        ));
    }

    #[test]
    fn later_foreign_tail_corroboration_preserves_already_displayed_text() {
        // Live session a12c722f: provider diarization split the synthetic room
        // speaker into a stable speaker-1 tail, while CAM++ produced two clean
        // 500/400 ms windows at 0.045/0.068. Each window is too short for hard
        // transcript exclusion, but together they corroborate that provider
        // boundary. The old final merge restored the polluted 53-char ledger
        // after the arbiter had selected the 43-char owner result.
        let owner =
            "开始录音。主人第一句，我会自然停顿一下。现在继续说主人第二句，最后这句话也不能丢。";
        let foreign = "这是一段来自另外一个。";
        let result = json!({
            "text": format!("{owner}{foreign}"),
            "utterances": [
                {
                    "additions": { "speaker_id": "0", "source": "two_pass" },
                    "definite": true,
                    "start_time": 172,
                    "end_time": 7_572,
                    "text": owner
                },
                {
                    "additions": { "speaker_id": "1", "source": "two_pass" },
                    "definite": true,
                    "start_time": 7_772,
                    "end_time": 9_902,
                    "text": foreign
                }
            ]
        });
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
            state.target_speaker_id = Some("0".into());
            state.best_transcript_text = format!("{owner}{foreign}");
            state.optimistic_preview_text = format!("{owner}{foreign}");
            state.last_partial_text = format!("{owner}{foreign}");
            state.last_emitted_preview_text = format!("{owner}{foreign}");
        }
        asr.note_local_speaker_observation(
            9_300,
            crate::speaker_verification::SessionSpeakerClassification::Uncertain { score: 0.045 },
            crate::speech_decision_kernel::TranscriptSpeakerEvidence::ForeignTailHint,
        );
        asr.note_local_speaker_observation(
            9_700,
            crate::speaker_verification::SessionSpeakerClassification::Uncertain { score: 0.068 },
            crate::speech_decision_kernel::TranscriptSpeakerEvidence::ForeignTailHint,
        );
        assert_eq!(
            asr.state
                .lock()
                .local_confirmed_transcript_foreign_hint_end_ms,
            Some(9_700)
        );

        let (tx, mut rx) = oneshot::channel();
        asr.state.lock().final_tx = Some(tx);
        let payload = serde_json::to_vec(&json!({
            "audio_info": { "duration": 9_900 },
            "result": result,
        }))
        .expect("short foreign-tail replay serializes");
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
            .expect("short foreign-tail final should resolve")
            .expect("filtered owner final should remain successful");
        // Speaker-filtered final authority must remain authoritative. The
        // historical preview may already contain the foreign tail, but that
        // old unfiltered text must not be copied back into the sealed result.
        assert_eq!(transcript.text, owner);
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
    fn degraded_tail_requires_confirmed_identity_departure() {
        let rows = vec![
            json!({
                "additions": { "speaker_id": "0", "source": "two_pass" },
                "definite": true,
                "start_time": 480,
                "end_time": 20_271,
                "text": "开始录音本人正文"
            }),
            json!({
                "additions": { "speaker_id": "0", "source": "two_pass" },
                "definite": true,
                "start_time": 20_271,
                "end_time": 28_872,
                "text": "旁人正文"
            }),
        ];
        let low = |audio_end_ms, score| LocalSpeakerEvidence {
            audio_end_ms,
            classification: crate::speaker_verification::SessionSpeakerClassification::NonTarget {
                score,
            },
            stable_target: true,
        };
        let ordinary_target = LocalSpeakerEvidence {
            audio_end_ms: 22_400,
            classification: crate::speaker_verification::SessionSpeakerClassification::Target {
                score: 0.59,
            },
            stable_target: true,
        };
        let evidence = vec![low(21_600, 0.05), ordinary_target, low(29_200, 0.12)];
        assert_eq!(
            degraded_same_cluster_foreign_tail_start(
                &rows,
                Some("0"),
                &evidence,
                true,
                false,
                true,
            ),
            None,
        );
        assert_eq!(
            degraded_same_cluster_foreign_tail_start(
                &rows,
                Some("0"),
                &evidence[..2],
                true,
                false,
                true,
            ),
            None,
            "one cross-phrase mismatch remains non-destructive",
        );
        let mut strong_owner = evidence;
        strong_owner.push(LocalSpeakerEvidence {
            audio_end_ms: 24_000,
            classification: crate::speaker_verification::SessionSpeakerClassification::Target {
                score: 0.82,
            },
            stable_target: true,
        });
        assert_eq!(
            degraded_same_cluster_foreign_tail_start(
                &rows,
                Some("0"),
                &strong_owner,
                true,
                false,
                true,
            ),
            None,
            "a high-confidence owner window keeps the later sentence",
        );
        let mut departed = strong_owner;
        departed.push(LocalSpeakerEvidence {
            audio_end_ms: 25_000,
            classification: crate::speaker_verification::SessionSpeakerClassification::NonTarget {
                score: 0.06,
            },
            stable_target: false,
        });
        assert_eq!(
            degraded_same_cluster_foreign_tail_start(
                &rows,
                Some("0"),
                &departed,
                true,
                false,
                true,
            ),
            Some(1),
        );
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
        // Attribution semantics are tested from an already-anchored session:
        // first-speaker anchoring is no longer an identity source (see
        // anchor_abstains_when_tracking_lost_and_no_identity_source).
        asr.state.lock().target_speaker_id = Some("0".into());

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
    fn qualified_owner_activity_raw_callback_waits_for_classifier_coverage() {
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
        }
        let updates = Arc::new(ParkingMutex::new(Vec::new()));
        let updates_for_callback = Arc::clone(&updates);
        asr.set_target_speaker_update_callback(Some(Arc::new(move |update| {
            updates_for_callback.lock().push(update);
        })));
        use crate::speaker_verification::SessionSpeakerClassification as Classification;

        let first_owner_end_ms = 2_000;
        asr.note_local_audio_activity(first_owner_end_ms, true);
        let raw_before_classifier = updates.lock().last().cloned().expect("raw update");
        assert_eq!(raw_before_classifier.qualified_owner_speech_end_ms, None);
        assert!(!raw_before_classifier.qualified_owner_activity_advanced);

        asr.note_local_speaker_observation(
            first_owner_end_ms,
            Classification::Target { score: 0.80 },
            crate::speech_decision_kernel::TranscriptSpeakerEvidence::Inconclusive,
        );
        let first_qualified_update = updates
            .lock()
            .last()
            .cloned()
            .expect("qualified classifier update");
        assert_eq!(
            first_qualified_update.qualified_owner_speech_end_ms,
            Some(first_owner_end_ms)
        );
        assert!(first_qualified_update.qualified_owner_activity_advanced);
        assert_eq!(
            first_qualified_update.local_speaker_classification_kind,
            Some(LocalSpeakerClassificationKind::Target)
        );
        assert_eq!(
            first_qualified_update.local_speaker_observation_end_ms,
            Some(first_owner_end_ms)
        );

        let raw_tail_end_ms = first_owner_end_ms + 400;
        asr.note_local_audio_activity(raw_tail_end_ms, true);
        let raw_tail = updates.lock().last().cloned().expect("raw tail update");
        assert_eq!(
            raw_tail.qualified_owner_speech_end_ms,
            Some(first_owner_end_ms)
        );
        assert!(!raw_tail.qualified_owner_activity_advanced);
        assert_eq!(
            raw_tail.local_speaker_classification_kind,
            None,
            "raw capture beyond classifier coverage must not reuse the old Target label"
        );
        assert_eq!(raw_tail.local_speaker_signal_quality_sufficient, None);
        assert_eq!(
            raw_tail.local_speaker_observation_end_ms,
            Some(first_owner_end_ms)
        );

        asr.note_local_speaker_observation(
            raw_tail_end_ms,
            Classification::Target { score: 0.82 },
            crate::speech_decision_kernel::TranscriptSpeakerEvidence::Inconclusive,
        );
        let recovered_owner = updates
            .lock()
            .last()
            .cloned()
            .expect("recovered owner update");
        assert_eq!(
            recovered_owner.qualified_owner_speech_end_ms,
            Some(raw_tail_end_ms)
        );
        assert!(recovered_owner.qualified_owner_activity_advanced);
        assert_eq!(
            recovered_owner.local_speaker_classification_kind,
            Some(LocalSpeakerClassificationKind::Target)
        );
        assert_eq!(
            recovered_owner.local_speaker_signal_quality_sufficient,
            Some(true)
        );
        assert_eq!(
            recovered_owner.local_speaker_observation_end_ms,
            Some(raw_tail_end_ms)
        );
    }

    #[test]
    fn uncertain_band_does_not_refresh_owner_endpoint_clock() {
        // F3 fixture（2026-08-09 12:47:04 弱 Target 序列 0.426–0.58）：置信
        // 余量带判 Uncertain 后不刷新本人端点时钟、也不冻结稳定身份；只有
        // 确信 Target 恢复刷新。2026-09-23 重校后 0.42+ 已是确信 Target——
        // 该序列在新世界里走 Target 路径（见 owner_body_band_lifts_freeze）。
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

    /// 2026-09-23 71f64ac1 实锤的行为级回归：旁人插话触发 owner_isolation
    /// 冻结后，本人恢复说话（对"TTS 银行+当会话唤醒锚点"档案实测得分
    /// 0.42–0.54，重校后为确信 Target）必须解除冻结。旧线 0.55 下这些窗
    /// 全是 Uncertain → 冻结永不解除 → 续说落屏被 gate blocked
    /// (owner_isolation_frozen) 挡死 + 终稿尾巴丢弃。
    #[test]
    fn owner_body_band_lifts_freeze_after_recalibration() {
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
            // 旁人持续 NonTarget 后的冻结态（当晚 12:03:02 gate blocked 的前态）。
            state.owner_isolation_frozen = true;
            state.owner_isolation_ceiling_text = "已上屏的前缀".into();
        }
        use crate::speaker_verification::SessionSpeakerClassification as Classification;
        // 本人恢复说话：0.43/0.47（71f64ac1 重放序列）——重校后是 Target。
        // LOCAL_SPEAKER_SWITCH_CONFIRMATIONS=2 个连续 Target 窗即可解冻。
        asr.note_local_speaker_classification(4_000, Classification::Target { score: 0.43 });
        asr.note_local_speaker_classification(4_400, Classification::Target { score: 0.47 });
        {
            let state = asr.state.lock();
            assert!(
                !state.owner_isolation_frozen,
                "owner resuming in the 0.42-0.54 body band must lift the isolation freeze"
            );
            assert!(state.owner_isolation_ceiling_text.is_empty());
        }
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
            crate::speech_decision_kernel::TranscriptSpeakerEvidence::HardNonTarget,
        );
        assert!(!asr.state.lock().owner_isolation_frozen);
        asr.note_local_speaker_observation(
            6_350,
            Classification::Uncertain { score: 0.077 },
            crate::speech_decision_kernel::TranscriptSpeakerEvidence::HardNonTarget,
        );

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
    fn provider_close_and_error_fallback_use_only_qualified_candidates() {
        // Exercise the production close/error fallback seam, not only the
        // ledger primitive: an accepted session candidate survives either
        // transport terminal, while a raw provider revision is used only
        // when no local ownership filter is active.
        for error in [
            VolcengineASRError::NoFinalResult,
            VolcengineASRError::ConnectionFailed("socket closed".into()),
        ] {
            let asr = VolcengineStreamingASR::new(
                VolcengineCredentials {
                    app_id: "app".into(),
                    access_token: "token".into(),
                    resource_id: VolcengineCredentials::default_resource_id().into(),
                },
                Vec::new(),
            );
            let (tx, mut rx) = oneshot::channel();
            {
                let mut state = asr.state.lock();
                state.best_transcript_text = "已接受的主人正文".into();
                state.last_partial_text = state.best_transcript_text.clone();
                state.transcript_evidence.note_provider_revision(
                    "已接受的主人正文",
                    Some(4_000),
                    false,
                    false,
                );
                state.final_tx = Some(tx);
            }

            asr.fallback_to_partial_or_error(error);
            let transcript = rx
                .try_recv()
                .expect("transport terminal must resolve the production fallback")
                .expect("accepted session text must survive transport terminal");
            assert_eq!(transcript.text, "已接受的主人正文");
        }

        let raw_only = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        let (tx, mut rx) = oneshot::channel();
        {
            let mut state = raw_only.state.lock();
            state.transcript_evidence.note_provider_revision(
                "未启用本地过滤时的 provider 正文",
                Some(4_200),
                false,
                false,
            );
            state.final_tx = Some(tx);
        }
        raw_only.fallback_to_partial_or_error(VolcengineASRError::NoFinalResult);
        let transcript = rx
            .try_recv()
            .expect("raw provider fallback must resolve")
            .expect("legal raw provider fallback must remain available");
        assert_eq!(transcript.text, "未启用本地过滤时的 provider 正文");
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
        assert_eq!(final_audio_drain_budget(200), Duration::from_secs(20));
        assert_eq!(final_audio_drain_budget(500), FINAL_AUDIO_DRAIN_MAX_BUDGET);
    }

    #[test]
    fn full_audio_replay_timeout_covers_the_long_recording_drain() {
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        // The live failure queued 2,149,120 bytes (67.2 seconds of audio).
        asr.retained_pcm.lock().resize(2_149_120, 0);
        assert!(asr.full_audio_replay_timeout() > Duration::from_secs(60));
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

    #[test]
    fn retained_audio_burst_preserves_every_sample_and_sequence() {
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        let (tx, mut rx) = mpsc::unbounded_channel();
        *asr.audio_tx.lock() = Some(tx);
        {
            let mut state = asr.state.lock();
            state.is_connected = true;
            state.next_sequence = 2;
        }
        let pcm: Vec<u8> = (0..TARGET_AUDIO_CHUNK_BYTES * 57)
            .map(|index| (index % 251) as u8)
            .collect();
        asr.consume_pcm_chunk(&pcm);
        let mut received = Vec::new();
        for expected_seq in 2..59 {
            let (seq, chunk) = rx.try_recv().expect("every audio frame must be queued");
            assert_eq!(seq, expected_seq);
            received.extend_from_slice(&chunk);
        }
        assert_eq!(received, pcm);
        assert!(rx.try_recv().is_err());
        assert_eq!(asr.pending_sends.load(Ordering::SeqCst), 57);
        assert_eq!(asr.state.lock().next_sequence, 59);
    }

    #[test]
    fn queued_audio_observation_stays_with_original_segment_after_rebind() {
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        let segment_a_guard = crate::observability::begin_embedded_audio_pipeline_capture(201);
        let segment_a = segment_a_guard.observation();
        segment_a.bind_sessions(None, Some(11));
        segment_a.record_asr_queued_for_segment(Some(11), 3_200);
        asr.queued_audio_observations.lock().insert(
            (asr.asr_stream_id(), 7),
            vec![AudioSourceShare {
                observation: Some(Arc::clone(&segment_a)),
                segment_id: Some(11),
                bytes: 3_200,
                destination_range: Some(crate::observability::PcmRange {
                    start: 0,
                    end: 3_200,
                }),
                source_interval: None,
            }],
        );

        let segment_b_guard = crate::observability::begin_embedded_audio_pipeline_capture(202);
        let segment_b = segment_b_guard.observation();
        segment_b.bind_sessions(None, Some(12));
        asr.set_pipeline_observation(Arc::clone(&segment_b));

        let source_shares = asr
            .take_queued_audio_observation(7)
            .expect("queued source must be retained by sequence");
        record_asr_send_completed_for_source_shares(
            asr.asr_stream_id(),
            7,
            destination_range_for_source_shares(&source_shares, 3_200),
            &source_shares,
            3_200,
        );

        assert_eq!(
            segment_a.asr_delivery_counts_for_test(11),
            (3_200, 3_200, 0, 0)
        );
        assert_eq!(segment_b.asr_delivery_counts_for_test(12), (0, 0, 0, 0));
    }

    #[test]
    fn production_consume_path_preserves_mixed_segment_shares_across_rebind() {
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        let (tx, mut rx) = mpsc::unbounded_channel();
        *asr.audio_tx.lock() = Some(tx);
        {
            let mut state = asr.state.lock();
            state.is_connected = true;
            state.next_sequence = 1;
        }

        let capture_guard = crate::observability::begin_embedded_audio_pipeline_capture(203);
        let capture_observation = capture_guard.observation();
        capture_observation.bind_sessions(None, Some(11));
        let rebound_guard = crate::observability::begin_embedded_audio_pipeline_capture(204);
        let rebound_observation = rebound_guard.observation();

        let first = vec![1_u8; 2_000];
        let second = vec![2_u8; 2_000];
        let third = vec![3_u8; 2_400];
        let first_interval = capture_observation
            .allocate_source_interval(1_100, Some(11), first.len())
            .expect("first source interval allocation");
        let second_interval = capture_observation
            .allocate_source_interval(1_100, Some(12), second.len())
            .expect("second source interval allocation");
        let third_interval = capture_observation
            .allocate_source_interval(1_100, Some(12), third.len())
            .expect("third source interval allocation");
        AudioConsumer::consume_pcm_chunk_with_source_interval(
            &asr,
            &first,
            Some(Arc::clone(&capture_observation)),
            Some(11),
            Some(first_interval),
        );
        AudioConsumer::consume_pcm_chunk_with_source_interval(
            &asr,
            &second,
            Some(Arc::clone(&capture_observation)),
            Some(12),
            Some(second_interval),
        );
        AudioConsumer::consume_pcm_chunk_with_source_interval(
            &asr,
            &third,
            Some(Arc::clone(&capture_observation)),
            Some(12),
            Some(third_interval),
        );
        asr.set_pipeline_observation(Arc::clone(&rebound_observation));

        let queued_facts = capture_observation.asr_destination_facts_for_test();
        assert_eq!(queued_facts.len(), 2);
        assert!(queued_facts
            .iter()
            .all(|fact| fact.asr_stream_id == asr.asr_stream_id()));
        assert_eq!(
            queued_facts[0].outcome,
            crate::observability::AsrDestinationOutcome::Queued
        );
        assert_eq!(
            queued_facts[0].destination_range,
            Some(crate::observability::PcmRange {
                start: 0,
                end: 3_200
            })
        );
        assert_eq!(
            queued_facts[1].destination_range,
            Some(crate::observability::PcmRange {
                start: 3_200,
                end: 6_400,
            })
        );

        let (_, first_frame) = rx.try_recv().expect("first mixed frame must be queued");
        let first_sources = asr
            .take_queued_audio_observation(1)
            .expect("first frame source must be retained");
        let (_, second_frame) = rx.try_recv().expect("second frame must be queued");
        let second_sources = asr
            .take_queued_audio_observation(2)
            .expect("second frame source must be retained");

        assert_eq!(
            first_sources
                .iter()
                .map(|share| (
                    share.segment_id,
                    share.bytes,
                    share.destination_range,
                    share.source_interval.map(|interval| interval.range),
                ))
                .collect::<Vec<_>>(),
            vec![
                (
                    Some(11),
                    2_000,
                    Some(crate::observability::PcmRange {
                        start: 0,
                        end: 2_000
                    }),
                    Some(crate::observability::PcmRange {
                        start: 0,
                        end: 2_000
                    }),
                ),
                (
                    Some(12),
                    1_200,
                    Some(crate::observability::PcmRange {
                        start: 2_000,
                        end: 3_200,
                    }),
                    Some(crate::observability::PcmRange {
                        start: 0,
                        end: 1_200
                    }),
                ),
            ]
        );
        assert_eq!(
            second_sources
                .iter()
                .map(|share| (
                    share.segment_id,
                    share.bytes,
                    share.destination_range,
                    share.source_interval.map(|interval| interval.range),
                ))
                .collect::<Vec<_>>(),
            vec![(
                Some(12),
                3_200,
                Some(crate::observability::PcmRange {
                    start: 3_200,
                    end: 6_400,
                }),
                Some(crate::observability::PcmRange {
                    start: 1_200,
                    end: 4_400
                }),
            )]
        );

        let mut received = first_frame;
        received.extend_from_slice(&second_frame);
        let mut expected = first;
        expected.extend_from_slice(&second);
        expected.extend_from_slice(&third);
        assert_eq!(received, expected);
        record_asr_claimed_for_source_shares(
            asr.asr_stream_id(),
            1,
            destination_range_for_source_shares(&first_sources, 3_200),
            &first_sources,
        );
        record_asr_send_completed_for_source_shares(
            asr.asr_stream_id(),
            1,
            destination_range_for_source_shares(&first_sources, 3_200),
            &first_sources,
            3_200,
        );
        record_asr_send_completed_for_source_shares(
            asr.asr_stream_id(),
            2,
            destination_range_for_source_shares(&second_sources, 3_200),
            &second_sources,
            3_200,
        );

        assert_eq!(
            capture_observation.asr_delivery_counts_for_test(11),
            (2_000, 2_000, 0, 0)
        );
        assert_eq!(
            capture_observation.asr_delivery_counts_for_test(12),
            (4_400, 4_400, 0, 0)
        );
        assert_eq!(
            rebound_observation.asr_delivery_counts_for_test(12),
            (0, 0, 0, 0)
        );
        assert!(capture_observation
            .asr_destination_facts_for_test()
            .iter()
            .all(|fact| fact.outcome
                == crate::observability::AsrDestinationOutcome::SocketSendCompleted));
    }

    #[test]
    fn missing_source_interval_keeps_destination_fact_unknown_without_borrowing_current_source() {
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        let (tx, mut rx) = mpsc::unbounded_channel();
        *asr.audio_tx.lock() = Some(tx);
        {
            let mut state = asr.state.lock();
            state.is_connected = true;
            state.next_sequence = 1;
        }
        let guard = crate::observability::begin_embedded_audio_pipeline_capture(224);
        let observation = guard.observation();

        AudioConsumer::consume_pcm_chunk_with_source(
            &asr,
            &[5_u8; TARGET_AUDIO_CHUNK_BYTES],
            Some(Arc::clone(&observation)),
            Some(74),
        );
        let _ = rx.try_recv().expect("unknown-source frame is still queued");
        let facts = observation.asr_destination_facts_for_test();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].source_shares.len(), 1);
        assert_eq!(facts[0].source_shares[0].source_interval, None);
        assert_eq!(
            facts[0].destination_range,
            Some(crate::observability::PcmRange {
                start: 0,
                end: TARGET_AUDIO_CHUNK_BYTES as u64,
            })
        );
        assert_eq!(
            facts[0].source_shares[0].destination_range,
            facts[0].destination_range
        );
    }

    #[test]
    fn independent_asr_sessions_do_not_collide_when_sequences_restart() {
        let credentials = VolcengineCredentials {
            app_id: "app".into(),
            access_token: "token".into(),
            resource_id: VolcengineCredentials::default_resource_id().into(),
        };
        let first = VolcengineStreamingASR::new(credentials.clone(), Vec::new());
        let second = VolcengineStreamingASR::new(credentials, Vec::new());
        assert_ne!(first.asr_stream_id(), second.asr_stream_id());
        assert_eq!(first.state.lock().next_sequence, 0);
        assert_eq!(second.state.lock().next_sequence, 0);
    }

    #[test]
    fn reopen_barrier_does_not_abandon_new_stream_or_claimed_old_stream() {
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        let old_id = asr.asr_stream_id();
        let old_guard = crate::observability::begin_embedded_audio_pipeline_capture(225);
        let old_observation = old_guard.observation();
        let old_share = vec![AudioSourceShare {
            observation: Some(Arc::clone(&old_observation)),
            segment_id: Some(81),
            bytes: TARGET_AUDIO_CHUNK_BYTES,
            destination_range: Some(crate::observability::PcmRange {
                start: 0,
                end: TARGET_AUDIO_CHUNK_BYTES as u64,
            }),
            source_interval: None,
        }];
        asr.queued_audio_observations
            .lock()
            .insert((old_id, 1), old_share.clone());
        let claimed = asr.claim_audio_source_for_worker(old_id, 1, TARGET_AUDIO_CHUNK_BYTES);
        record_asr_claimed_for_source_shares(
            old_id,
            1,
            destination_range_for_source_shares(&claimed, TARGET_AUDIO_CHUNK_BYTES),
            &claimed,
        );

        let new_id = old_id.saturating_add(1);
        asr.asr_stream_id.store(new_id, Ordering::Relaxed);
        let new_guard = crate::observability::begin_embedded_audio_pipeline_capture(226);
        let new_observation = new_guard.observation();
        asr.queued_audio_observations.lock().insert(
            (new_id, 1),
            vec![AudioSourceShare {
                observation: Some(Arc::clone(&new_observation)),
                segment_id: Some(82),
                bytes: TARGET_AUDIO_CHUNK_BYTES,
                destination_range: Some(crate::observability::PcmRange {
                    start: 0,
                    end: TARGET_AUDIO_CHUNK_BYTES as u64,
                }),
                source_interval: None,
            }],
        );

        // The old claimed frame is owned by its worker and the new queued
        // frame belongs to the rebinding stream. Cleanup must touch neither.
        assert_eq!(asr.abandon_queued_audio_observations(old_id), 0);
        assert!(asr
            .queued_audio_observations
            .lock()
            .contains_key(&(new_id, 1)));
        record_asr_send_completed_for_source_shares(
            old_id,
            1,
            destination_range_for_source_shares(&claimed, TARGET_AUDIO_CHUNK_BYTES),
            &claimed,
            TARGET_AUDIO_CHUNK_BYTES,
        );
        assert_eq!(
            old_observation.asr_destination_facts_for_test()[0].outcome,
            crate::observability::AsrDestinationOutcome::SocketSendCompleted
        );
        assert_eq!(asr.abandon_queued_audio_observations(new_id), 1);
        assert_eq!(
            new_observation.asr_destination_facts_for_test()[0].outcome,
            crate::observability::AsrDestinationOutcome::Abandoned
        );
        asr.release_in_flight_audio_observation(old_id, 1);
    }

    #[test]
    fn reopen_barrier_abandons_only_unclaimed_old_stream_entries() {
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        let old_id = asr.asr_stream_id();
        let old_guard = crate::observability::begin_embedded_audio_pipeline_capture(227);
        let old_observation = old_guard.observation();
        asr.queued_audio_observations.lock().insert(
            (old_id, 2),
            vec![AudioSourceShare {
                observation: Some(Arc::clone(&old_observation)),
                segment_id: Some(83),
                bytes: 2_000,
                destination_range: Some(crate::observability::PcmRange {
                    start: 3_200,
                    end: 5_200,
                }),
                source_interval: None,
            }],
        );
        let new_id = old_id.saturating_add(1);
        asr.asr_stream_id.store(new_id, Ordering::Relaxed);
        let new_guard = crate::observability::begin_embedded_audio_pipeline_capture(228);
        let new_observation = new_guard.observation();
        asr.queued_audio_observations.lock().insert(
            (new_id, 2),
            vec![AudioSourceShare {
                observation: Some(Arc::clone(&new_observation)),
                segment_id: Some(84),
                bytes: 2_000,
                destination_range: Some(crate::observability::PcmRange {
                    start: 0,
                    end: 2_000,
                }),
                source_interval: None,
            }],
        );

        assert_eq!(asr.abandon_queued_audio_observations(old_id), 1);
        assert!(!asr
            .queued_audio_observations
            .lock()
            .contains_key(&(old_id, 2)));
        assert!(asr
            .queued_audio_observations
            .lock()
            .contains_key(&(new_id, 2)));
        assert_eq!(
            old_observation.asr_destination_facts_for_test()[0].outcome,
            crate::observability::AsrDestinationOutcome::Abandoned
        );
        assert!(new_observation.asr_destination_facts_for_test().is_empty());
        assert_eq!(asr.abandon_queued_audio_observations(new_id), 1);
    }

    #[test]
    fn mismatched_interval_is_local_to_one_call_and_cannot_bind_the_next_pcm() {
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
            state.is_connected = true;
        }
        let guard = crate::observability::begin_embedded_audio_pipeline_capture(212);
        let observation = guard.observation();
        let prior = observation
            .allocate_source_interval(1_177, Some(77), 100)
            .expect("prior interval");
        AudioConsumer::consume_pcm_chunk_with_source_interval(
            &asr,
            &[1_u8; 101],
            Some(Arc::clone(&observation)),
            Some(77),
            Some(prior),
        );

        let next = observation
            .allocate_source_interval(1_177, Some(77), 100)
            .expect("next interval remains independently allocatable");
        AudioConsumer::consume_pcm_chunk_with_source_interval(
            &asr,
            &[2_u8; 100],
            Some(Arc::clone(&observation)),
            Some(77),
            Some(next),
        );

        let (bytes, destination_range, shares) = asr.take_pending_audio();
        assert_eq!(bytes, 201);
        assert_eq!(
            destination_range,
            Some(crate::observability::PcmRange { start: 0, end: 201 })
        );
        assert_eq!(shares.len(), 2);
        assert_eq!(shares[0].bytes, 101);
        assert!(shares[0].source_interval.is_none());
        assert_eq!(shares[1].bytes, 100);
        assert_eq!(
            shares[1].source_interval.map(|interval| interval.range),
            Some(crate::observability::PcmRange {
                start: 100,
                end: 200
            })
        );
        assert!(observation.interval_ledger_incomplete_for_test());
    }

    #[test]
    fn source_interval_validation_is_local_and_rejects_wrong_generation_segment_or_observation() {
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
            state.is_connected = true;
        }
        let first_guard = crate::observability::begin_embedded_audio_pipeline_capture(220);
        let first_observation = first_guard.observation();
        let second_guard = crate::observability::begin_embedded_audio_pipeline_capture(221);
        let second_observation = second_guard.observation();

        let wrong_generation = first_observation
            .allocate_source_interval(2_201, Some(7), 4)
            .expect("wrong-generation interval");
        AudioConsumer::consume_pcm_chunk_with_source_interval(
            &asr,
            &[1_u8; 4],
            Some(Arc::clone(&second_observation)),
            Some(7),
            Some(wrong_generation),
        );

        let wrong_segment = first_observation
            .allocate_source_interval(2_201, Some(8), 4)
            .expect("wrong-segment interval");
        AudioConsumer::consume_pcm_chunk_with_source_interval(
            &asr,
            &[2_u8; 4],
            Some(Arc::clone(&first_observation)),
            Some(9),
            Some(wrong_segment),
        );

        let missing_observation = first_observation
            .allocate_source_interval(2_201, Some(10), 4)
            .expect("missing-observation interval");
        AudioConsumer::consume_pcm_chunk_with_source_interval(
            &asr,
            &[3_u8; 4],
            None,
            Some(10),
            Some(missing_observation),
        );

        let valid = first_observation
            .allocate_source_interval(2_201, Some(11), 4)
            .expect("valid interval after invalid calls");
        AudioConsumer::consume_pcm_chunk_with_source_interval(
            &asr,
            &[4_u8; 4],
            Some(Arc::clone(&first_observation)),
            Some(11),
            Some(valid),
        );

        let (pcm, destination_range, shares) = asr.take_pending_audio();
        assert_eq!(pcm, 16);
        assert_eq!(
            destination_range,
            Some(crate::observability::PcmRange { start: 0, end: 16 })
        );
        assert_eq!(shares.len(), 4);
        assert!(shares[..3]
            .iter()
            .all(|share| share.source_interval.is_none()));
        assert_eq!(
            shares[3].source_interval.map(|interval| interval.range),
            Some(crate::observability::PcmRange { start: 0, end: 4 })
        );
        assert!(first_observation.interval_ledger_incomplete_for_test());
        assert!(second_observation.interval_ledger_incomplete_for_test());
    }

    #[test]
    fn two_logical_streams_keep_independent_ranges_in_asr_source_shares() {
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
            state.is_connected = true;
        }
        let guard = crate::observability::begin_embedded_audio_pipeline_capture(222);
        let observation = guard.observation();
        let first = observation
            .allocate_source_interval(2_221, Some(7), 4)
            .expect("first logical stream interval");
        let second = observation
            .allocate_source_interval(2_222, Some(7), 4)
            .expect("second logical stream interval");

        AudioConsumer::consume_pcm_chunk_with_source_interval(
            &asr,
            &[1_u8; 4],
            Some(Arc::clone(&observation)),
            Some(7),
            Some(first),
        );
        AudioConsumer::consume_pcm_chunk_with_source_interval(
            &asr,
            &[2_u8; 4],
            Some(Arc::clone(&observation)),
            Some(7),
            Some(second),
        );

        let (_, _, shares) = asr.take_pending_audio();
        assert_eq!(shares.len(), 2);
        assert_eq!(shares[0].source_interval.unwrap().source_stream_id, 2_221);
        assert_eq!(shares[1].source_interval.unwrap().source_stream_id, 2_222);
        assert_eq!(shares[0].source_interval.unwrap().range.start, 0);
        assert_eq!(shares[1].source_interval.unwrap().range.start, 0);
    }

    #[test]
    fn explicit_unknown_source_does_not_fallback_to_current_observation() {
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        let (tx, mut rx) = mpsc::unbounded_channel();
        *asr.audio_tx.lock() = Some(tx);
        {
            let mut state = asr.state.lock();
            state.is_connected = true;
            state.next_sequence = 1;
        }
        let guard = crate::observability::begin_embedded_audio_pipeline_capture(208);
        let old_observation = guard.observation();
        asr.set_pipeline_observation(Arc::clone(&old_observation));
        let pcm = vec![13_u8; TARGET_AUDIO_CHUNK_BYTES];

        AudioConsumer::consume_pcm_chunk_with_source(&asr, &pcm, None, Some(73));

        let (_, delivered) = rx
            .try_recv()
            .expect("explicitly unknown PCM is still delivered");
        assert_eq!(delivered, pcm);
        assert_eq!(
            old_observation.asr_delivery_counts_for_test(73),
            (0, 0, 0, 0)
        );
    }

    #[test]
    fn missing_worker_source_stays_unassociated_across_capture_rebind() {
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        let old_capture = crate::observability::begin_embedded_audio_pipeline_capture(210);
        let old_observation = old_capture.observation();
        asr.set_pipeline_observation(Arc::clone(&old_observation));

        let source_shares =
            asr.claim_audio_source_for_worker(asr.asr_stream_id(), 1, TARGET_AUDIO_CHUNK_BYTES);

        assert_eq!(source_shares.len(), 1);
        assert!(source_shares[0].observation.is_none());
        assert_eq!(
            old_observation.asr_delivery_counts_for_test(75),
            (0, 0, 0, 0)
        );
        let diagnostic = asr.take_diagnostic_trace();
        assert_eq!(diagnostic.unassociated_audio_frame_count, 1);
        assert_eq!(
            diagnostic.unassociated_audio_pcm_bytes,
            TARGET_AUDIO_CHUNK_BYTES
        );
    }

    #[test]
    fn cancel_seals_a_pending_half_frame_as_unresolved_under_one_boundary() {
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        let (tx, _rx) = mpsc::unbounded_channel();
        *asr.audio_tx.lock() = Some(tx);
        {
            let mut state = asr.state.lock();
            state.is_connected = true;
            state.next_sequence = 1;
        }
        let capture = crate::observability::begin_embedded_audio_pipeline_capture(211);
        let observation = capture.observation();

        AudioConsumer::consume_pcm_chunk_with_source(
            &asr,
            &[3_u8; 640],
            Some(Arc::clone(&observation)),
            Some(76),
        );
        asr.cancel();

        assert!(asr.state.lock().pending_audio.is_empty());
        assert_eq!(
            observation.asr_terminal_counts_for_test(76),
            (0, 0, 0, 640, 0, 0)
        );
    }

    #[test]
    fn claimed_audio_source_is_not_abandoned_by_cancel_cleanup() {
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        let guard = crate::observability::begin_embedded_audio_pipeline_capture(209);
        let observation = guard.observation();
        let shares = vec![AudioSourceShare {
            observation: Some(Arc::clone(&observation)),
            segment_id: Some(74),
            bytes: TARGET_AUDIO_CHUNK_BYTES,
            destination_range: Some(crate::observability::PcmRange {
                start: 0,
                end: TARGET_AUDIO_CHUNK_BYTES as u64,
            }),
            source_interval: None,
        }];
        record_asr_queued_for_source_shares(
            asr.asr_stream_id(),
            1,
            Some(crate::observability::PcmRange {
                start: 0,
                end: TARGET_AUDIO_CHUNK_BYTES as u64,
            }),
            &shares,
            TARGET_AUDIO_CHUNK_BYTES,
        );
        asr.queued_audio_observations
            .lock()
            .insert((asr.asr_stream_id(), 1), shares);
        asr.pending_sends.store(1, Ordering::SeqCst);

        let claimed = asr
            .claim_queued_audio_observation(1)
            .expect("worker claims the frame before its first await");
        record_asr_claimed_for_source_shares(
            asr.asr_stream_id(),
            1,
            destination_range_for_source_shares(&claimed, TARGET_AUDIO_CHUNK_BYTES),
            &claimed,
        );
        assert_eq!(
            asr.abandon_queued_audio_observations(asr.asr_stream_id()),
            0
        );
        assert_eq!(observation.asr_terminal_counts_for_test(74).2, 0);

        asr.cancel();
        asr.release_in_flight_audio_observation(asr.asr_stream_id(), 1);
        record_asr_send_completed_for_source_shares(
            asr.asr_stream_id(),
            1,
            destination_range_for_source_shares(&claimed, TARGET_AUDIO_CHUNK_BYTES),
            &claimed,
            TARGET_AUDIO_CHUNK_BYTES,
        );

        assert_eq!(
            observation.asr_terminal_counts_for_test(74),
            (TARGET_AUDIO_CHUNK_BYTES as u64, 0, 0, 0, 0, 0)
        );
        assert_eq!(
            observation.asr_destination_facts_for_test()[0].outcome,
            crate::observability::AsrDestinationOutcome::SocketSendCompleted
        );
    }

    #[test]
    fn failed_and_unresolved_audio_are_source_bound_and_settled_once() {
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        let (tx, mut rx) = mpsc::unbounded_channel();
        *asr.audio_tx.lock() = Some(tx);
        {
            let mut state = asr.state.lock();
            state.is_connected = true;
            state.next_sequence = 1;
        }
        let guard = crate::observability::begin_embedded_audio_pipeline_capture(205);
        let observation = guard.observation();

        AudioConsumer::consume_pcm_chunk_with_source(
            &asr,
            &vec![7_u8; TARGET_AUDIO_CHUNK_BYTES],
            Some(Arc::clone(&observation)),
            Some(71),
        );
        let _ = rx.try_recv().expect("fake transport receives one frame");
        let failed_sources = asr
            .take_queued_audio_observation(1)
            .expect("failed frame source must be retained");
        let replacement = crate::observability::begin_embedded_audio_pipeline_capture(206);
        asr.set_pipeline_observation(replacement.observation());
        record_asr_send_failed_for_source_shares(
            asr.asr_stream_id(),
            1,
            destination_range_for_source_shares(&failed_sources, TARGET_AUDIO_CHUNK_BYTES),
            &failed_sources,
            TARGET_AUDIO_CHUNK_BYTES,
        );

        AudioConsumer::consume_pcm_chunk_with_source(
            &asr,
            &[8_u8; 640],
            Some(Arc::clone(&observation)),
            Some(71),
        );
        asr.abandon_pending_audio();
        asr.abandon_pending_audio();

        assert_eq!(
            observation.asr_terminal_counts_for_test(71),
            (0, TARGET_AUDIO_CHUNK_BYTES as u64, 0, 640, 0, 0)
        );
        assert_eq!(
            replacement.observation().asr_terminal_counts_for_test(71),
            (0, 0, 0, 0, 0, 0)
        );
        let facts = observation.asr_destination_facts_for_test();
        assert!(facts.iter().any(|fact| {
            fact.sequence == Some(1)
                && fact.outcome == crate::observability::AsrDestinationOutcome::SendFailed
        }));
        assert!(facts.iter().any(|fact| {
            fact.sequence.is_none()
                && fact.outcome == crate::observability::AsrDestinationOutcome::BufferedUnresolved
        }));
    }

    #[test]
    fn closed_audio_queue_rejects_every_unsent_frame_with_its_source() {
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        let (tx, rx) = mpsc::unbounded_channel();
        *asr.audio_tx.lock() = Some(tx);
        drop(rx);
        {
            let mut state = asr.state.lock();
            state.is_connected = true;
            state.next_sequence = 1;
        }
        let guard = crate::observability::begin_embedded_audio_pipeline_capture(207);
        let observation = guard.observation();

        AudioConsumer::consume_pcm_chunk_with_source(
            &asr,
            &vec![9_u8; TARGET_AUDIO_CHUNK_BYTES * 2],
            Some(Arc::clone(&observation)),
            Some(72),
        );

        assert_eq!(asr.pending_sends.load(Ordering::SeqCst), 0);
        assert_eq!(
            observation.asr_terminal_counts_for_test(72),
            (0, 0, 0, 0, 0, (TARGET_AUDIO_CHUNK_BYTES * 2) as u64)
        );
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

    #[test]
    fn stalled_stream_uses_ledger_only_when_it_covers_the_last_owner_speech() {
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
        );
        {
            let mut st = asr.state.lock();
            st.audio_delivery_readiness = AudioDeliveryReadiness::Failed(
                VolcengineASRError::ConnectionFailed("uplink stall: server went silent".into()),
            );
            st.best_transcript_text = "停顿以后继续说的完整句子。".into();
            st.best_transcript_committed_at = Some(Instant::now() - Duration::from_secs(2));
            st.best_transcript_segments = vec![TranscriptSegment {
                start_ms: 0,
                end_ms: Some(29_200),
                text: st.best_transcript_text.clone(),
            }];
            st.qualified_owner_speech_end_ms = Some(26_600);
            st.local_target_speech_end_ms = Some(26_600);
            st.local_non_target_speech_end_ms = Some(12_600);
            st.last_server_audio_duration_ms = Some(29_800);
        }
        assert!(asr.stalled_ledger_covers_owner_speech());
        asr.state.lock().local_target_speech_end_ms = Some(31_000);
        assert!(!asr.stalled_ledger_covers_owner_speech(), "new speech after cloud coverage still requires replay");
        asr.state.lock().local_target_speech_end_ms = Some(26_600);
        asr.state.lock().local_non_target_speech_end_ms = Some(30_000);
        assert!(!asr.stalled_ledger_covers_owner_speech(), "a later foreign speaker still requires arbitration");
    }

    #[tokio::test]
    async fn stalled_stream_finishes_from_the_existing_ledger_without_replay() {
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
            let mut st = asr.state.lock();
            st.final_tx = Some(tx);
            st.best_transcript_text = "停顿后我继续说了，最后一句也在。".into();
            st.best_transcript_committed_at = Some(Instant::now() - Duration::from_secs(2));
            st.best_transcript_segments = vec![TranscriptSegment {
                start_ms: 0,
                end_ms: Some(29_200),
                text: st.best_transcript_text.clone(),
            }];
            st.qualified_owner_speech_end_ms = Some(26_600);
            st.local_target_speech_end_ms = Some(26_600);
            st.local_non_target_speech_end_ms = Some(12_600);
            st.last_server_audio_duration_ms = Some(29_800);
        }
        *asr.final_rx.lock() = Some(rx);
        asr.fallback_to_partial_or_error(VolcengineASRError::ConnectionFailed(
            "uplink stall: audio kept flowing but server went silent".into(),
        ));
        assert!(asr.send_last_frame().await.is_ok());
        assert_eq!(
            asr.await_final_result_with_early_seal().await.unwrap().text,
            "停顿后我继续说了，最后一句也在。"
        );
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
        accelerate_score: u8,
        end_window_size_ms: Option<u32>,
        force_to_speech_time_ms: Option<u32>,
        result_type: String,
        callback_count: u64,
        first_callback_ms: Option<u64>,
        max_callback_gap_ms: Option<u64>,
        final_chars: Option<usize>,
        final_text: Option<String>,
        final_elapsed_ms: Option<u64>,
        error_kind: Option<String>,
        provider_responses: Vec<Value>,
    }

    #[derive(serde::Serialize)]
    struct ProviderCadenceProbeSummary {
        mode: &'static str,
        ssd_version_override: Option<String>,
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
        let accelerate_score = options.accelerate_score;
        let end_window_size_ms = options.end_window_size_ms;
        let force_to_speech_time_ms = options.force_to_speech_time_ms;
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
            asr.mark_audio_delivery_ready();
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
        let provider_responses = asr.state.lock().diagnostic_provider_responses.clone();
        match run {
            Ok(final_result) => ProviderCadenceProbeVariant {
                endpoint,
                enable_nonstream,
                accelerate_score,
                end_window_size_ms,
                force_to_speech_time_ms,
                result_type,
                callback_count: stats.callbacks,
                first_callback_ms: stats.first_callback_ms,
                max_callback_gap_ms: (stats.callbacks > 1).then_some(stats.max_callback_gap_ms),
                final_chars: Some(final_result.text.chars().count()),
                final_text: Some(final_result.text),
                final_elapsed_ms: Some(started.elapsed().as_millis() as u64),
                error_kind: None,
                provider_responses,
            },
            Err(error) => ProviderCadenceProbeVariant {
                endpoint,
                enable_nonstream,
                accelerate_score,
                end_window_size_ms,
                force_to_speech_time_ms,
                result_type,
                callback_count: stats.callbacks,
                first_callback_ms: stats.first_callback_ms,
                max_callback_gap_ms: (stats.callbacks > 1).then_some(stats.max_callback_gap_ms),
                final_chars: None,
                final_text: None,
                final_elapsed_ms: Some(started.elapsed().as_millis() as u64),
                error_kind: Some(error.to_string()),
                provider_responses,
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

        let variants =
            if std::env::var_os("LISTENER_PROVIDER_CADENCE_COMPARE_SEGMENTATION").is_some() {
                vec![
                    VolcengineSessionOptions {
                        force_to_speech_time_ms: Some(0),
                        end_window_size_ms: Some(500),
                        ..Default::default()
                    },
                    VolcengineSessionOptions {
                        force_to_speech_time_ms: Some(1_000),
                        end_window_size_ms: Some(500),
                        ..Default::default()
                    },
                    VolcengineSessionOptions {
                        force_to_speech_time_ms: Some(1_000),
                        end_window_size_ms: Some(1_000),
                        ..Default::default()
                    },
                ]
            } else {
                vec![
                    VolcengineSessionOptions {
                        accelerate_score: 0,
                        ..Default::default()
                    },
                    VolcengineSessionOptions {
                        endpoint: VolcengineSessionEndpoint::OptimizedBidirectional,
                        enable_nonstream: true,
                        accelerate_score: 10,
                        result_type: VolcengineResultType::Full,
                        end_window_size_ms: Some(SECOND_PASS_END_WINDOW_MS),
                        force_to_speech_time_ms: Some(SECOND_PASS_FORCE_TO_SPEECH_MS),
                    },
                ]
            };
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
                ssd_version_override: std::env::var("LISTENER_PROVIDER_CADENCE_SSD_VERSION").ok(),
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
                true,
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
        let target_embedding =
            crate::asr::target_speaker_extraction::speaker_embedding_from_enrollment_pcm(
                &owner_pcm,
            )
            .expect("encode public target-speaker enrollment");
        asr.start_target_speaker_extraction_with_embedding(target_embedding);
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
                        "transcriptSpeakerEvidence": observation.transcript_speaker_evidence.label(),
                    }));
                    asr.note_local_speaker_observation_with_quality(
                        audio_end_ms,
                        observation.classification,
                        observation.transcript_speaker_evidence,
                        observation.signal_quality_sufficient,
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
        let (primary, target) = tokio::join!(
            async {
                let result = asr
                    .await_final_result_with_timeout(Duration::from_secs(20))
                    .await
                    .expect("receive filtered final");
                (result, finalization_started.elapsed().as_millis())
            },
            async {
                let result = asr
                    .await_target_speaker_final()
                    .await
                    .expect("receive target-speaker selection result");
                (result, finalization_started.elapsed().as_millis())
            },
        );
        let (final_result, primary_final_elapsed_ms) = primary;
        let (target_result, target_final_elapsed_ms) = target;
        let selected_text = target_result
            .as_ref()
            .map(|target| target.text.as_str())
            .unwrap_or(final_result.text.as_str())
            .to_string();
        let spoken = |text: &str| {
            text.chars()
                .filter(|character| character.is_alphanumeric())
                .collect::<String>()
        };
        let passed = spoken(&selected_text) == spoken(&expected_text)
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
