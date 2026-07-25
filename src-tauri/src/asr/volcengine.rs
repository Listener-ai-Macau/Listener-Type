//! Volcengine SAUC bigmodel streaming ASR client.
//!
//! Direct port of the Swift `VolcengineStreamingASR`. Battle-tested protocol
//! quirks are preserved verbatim — see comments tagged with `[asr]` for the
//! original learnings (especially the "definite=true is NOT stream end" bug).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex as ParkingMutex;
use serde_json::{json, Value};
use tokio::net::TcpStream;
use tokio::runtime::Handle;
use tokio::sync::{mpsc, oneshot, Mutex as AsyncMutex, Notify};
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

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;
type WsSink = futures_util::stream::SplitSink<WsStream, Message>;
type SharedWriter = Arc<AsyncMutex<Option<WsSink>>>;
type PartialTranscriptCallback = Arc<dyn Fn(String) + Send + Sync>;
type FinalIntermediateTranscriptCallback = Arc<dyn Fn(FinalIntermediateTranscript) + Send + Sync>;
type StreamingEventCallback = Arc<dyn Fn(VolcengineStreamingEvent) + Send + Sync>;

#[derive(Clone, Debug)]
pub struct FinalIntermediateTranscript {
    pub text: String,
    pub authoritative_two_pass: bool,
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
    is_unstable_initial_partial, merge_streaming_candidate, normalize_cjk_final_spacing_and_echoes,
    normalized_result, transcript_candidate_from_result, trim_repeated_short_final_tail,
    trim_repeated_short_streaming_tail, TranscriptSegment,
};

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
    /// 最新服务端响应已处理的音频时长。two-pass 终帧可能只给最后一个 utterance 的
    /// 词时间戳，但 `audio_info.duration` 仍覆盖整段音频；收尾时应优先采用这项
    /// 传输级覆盖证据，避免把已返回的完整文本误判为截断。
    last_server_audio_duration_ms: Option<u64>,
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
    json!({
        "audio_duration_ms": server_audio_duration_ms(json),
        "sources": sources,
        "utterance_count": utterance_count,
        "has_final_frame": has_final_frame,
        "authoritative_two_pass": authoritative_two_pass,
        "result_chars": result
            .get("text")
            .and_then(Value::as_str)
            .map_or(0, |text| text.chars().count()),
    })
}

pub struct VolcengineStreamingASR {
    credentials: VolcengineCredentials,
    hotwords: Vec<DictionaryHotword>,
    session_options: VolcengineSessionOptions,
    state: ParkingMutex<SyncState>,
    partial_callback: ParkingMutex<Option<PartialTranscriptCallback>>,
    final_intermediate_callback: ParkingMutex<Option<FinalIntermediateTranscriptCallback>>,
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
            final_intermediate_callback: ParkingMutex::new(None),
            streaming_event_callback: ParkingMutex::new(None),
            writer: Arc::new(AsyncMutex::new(None)),
            final_rx: ParkingMutex::new(None),
            audio_tx: ParkingMutex::new(None),
            pending_sends: Arc::new(AtomicUsize::new(0)),
            pending_sends_high_water: Arc::new(AtomicUsize::new(0)),
            send_done: Arc::new(Notify::new()),
            audio_delivery_changed: Arc::new(Notify::new()),
        }
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

    fn mark_audio_delivery_failed(&self, error: VolcengineASRError) {
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

    pub fn set_final_intermediate_transcript_callback(
        &self,
        callback: Option<FinalIntermediateTranscriptCallback>,
    ) {
        *self.final_intermediate_callback.lock() = callback;
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

    fn emit_final_intermediate_transcript(&self, update: FinalIntermediateTranscript) {
        let callback = self.final_intermediate_callback.lock().clone();
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
        self.mark_audio_delivery_opening();
        let result = self.open_session_inner().await;
        if let Err(error) = &result {
            self.mark_audio_delivery_failed(error.clone());
        }
        result
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

        let (ws, _resp) = connect_async(request)
            .await
            .map_err(classify_connect_error)?;
        let (write, read) = ws.split();

        let (tx, rx) = oneshot::channel();

        // Reset sync state for the new session.
        {
            let mut st = self.state.lock();
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
            st.last_server_audio_duration_ms = None;
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
        self.state.lock().finishing = true;

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
            "enable_itn": true,
            "enable_punc": true,
            "show_utterances": true,
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
            self.fallback_to_partial_or_error(VolcengineASRError::ConnectionFailed(format!(
                "ASR error {}: {}",
                code, body
            )));
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

        // 流结束信号只信帧头 flags（lastPacket / negativeSequence）。
        // 之前误把 utterance.definite=true 当成流结束——但那只代表"这一段语音已固化"，
        // 用户可能还在继续说。结果一收到第一个 definite=true 就关掉接收，
        // 后面用户讲的内容全部丢失（实测丢了 9 秒）。
        let has_final = parsed.is_final();
        let candidate = transcript_candidate_from_result(result);
        let authoritative_two_pass = candidate.authoritative_cumulative;
        log::info!(
            "[asr] {} server metadata: {}",
            self.session_options.endpoint.label(),
            provider_response_metadata(&json, result, has_final, authoritative_two_pass)
        );
        if let Some(payload) = payload_for_log {
            let text_is_empty = candidate.text.trim().is_empty();
            if text_is_empty && !has_final {
                log::debug!(
                    "[asr] {} server JSON(empty): {}",
                    self.session_options.endpoint.label(),
                    payload
                );
            } else {
                log::info!(
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
        let (full_text, partial_changed) = {
            let mut state = self.state.lock();
            let (mut merged, segments) = merge_streaming_candidate(
                &state.best_transcript_text,
                &state.best_transcript_segments,
                candidate,
            );
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
            (merged, changed)
        };

        if !has_final && authoritative_two_pass && partial_changed && !full_text.is_empty() {
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
        if (has_final
            || self
                .session_options
                .endpoint
                .emits_stream_preview_before_final())
            && (partial_changed || has_final)
            && !full_text.is_empty()
        {
            let elapsed_ms = self
                .state
                .lock()
                .start
                .map(|s| s.elapsed().as_millis() as u64)
                .unwrap_or(0);
            {
                let mut st = self.state.lock();
                st.partial_updates_seen += 1;
            }
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

    /// 服务端 close / 网络中断时调用：如果有缓存的 partial 文本，作为 transcript
    /// 兜底返回；否则才报错。配合 `last_partial_text` 实现「至少不丢用户已识别出的话」。
    fn fallback_to_partial_or_error(&self, err: VolcengineASRError) {
        let delivery_error = err.clone();
        let (partial, duration_ms) = {
            let st = self.state.lock();
            (
                st.last_partial_text.clone(),
                st.start
                    .map(|s| s.elapsed().as_millis() as u64)
                    .unwrap_or(0),
            )
        };
        if !partial.is_empty() {
            log::warn!(
                "[asr] {}; 使用 partial 兜底（{} 字）",
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
        // 单 worker 串行 send 模式：在 state 锁内 drain 并分配 seq（seq 单调），
        // 然后把 (seq, chunk) push 进 mpsc。worker 端按入队顺序 send，
        // 哪怕跨多个 consume 调用、多个 spawn 也不会再有 writer 锁竞争。
        let chunks: Vec<(i32, Vec<u8>)> = {
            let mut st = self.state.lock();
            if !st.is_connected {
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
    fn default_resource_id_is_sauc_duration() {
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

    #[test]
    fn provider_response_metadata_excludes_transcript_text() {
        let json = json!({
            "audio_info": { "duration": 1_600 },
            "result": {
                "text": "private transcript",
                "utterances": [{
                    "additions": { "source": "two_pass" }
                }]
            }
        });
        let metadata = provider_response_metadata(&json, &json["result"], false, true);

        assert_eq!(metadata["audio_duration_ms"], 1_600);
        assert_eq!(metadata["sources"], json!(["two_pass"]));
        assert_eq!(metadata["utterance_count"], 1);
        assert_eq!(metadata["has_final_frame"], false);
        assert_eq!(metadata["authoritative_two_pass"], true);
        assert_eq!(metadata["result_chars"], 18);
        assert!(!metadata.to_string().contains("private transcript"));
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
        assert_eq!(request["enable_nonstream"], true);
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
    fn optimized_bidirectional_emits_its_early_stream_result_before_two_pass() {
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
        assert_eq!(&*previews.lock(), &["请在今天".to_string()]);
        assert_eq!(asr.state.lock().partial_updates_seen, 1);
    }

    #[test]
    fn final_frame_send_budget_stays_below_final_result_wait() {
        assert!(WEBSOCKET_SEND_TIMEOUT < FINAL_RESULT_TIMEOUT);
        assert!(FINAL_FRAME_SEND_BUDGET < FINAL_RESULT_TIMEOUT);
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
        assert_eq!(
            pcm.len(),
            60_000 * BYTES_PER_MS as usize,
            "probe PCM must be exactly 60,000 ms"
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
}
