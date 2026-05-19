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

const ENDPOINT: &str = "wss://openspeech.bytedance.com/api/v3/sauc/bigmodel_async";
/// 200 ms of 16 kHz / 16-bit / mono PCM.
const TARGET_AUDIO_CHUNK_BYTES: usize = 6_400;
/// 16 kHz · 16-bit · mono = 32 000 bytes/sec → 32 bytes/ms.
const BYTES_PER_MS: f64 = 32.0;
const HOTWORD_CAP: usize = 80;
const FINAL_RESULT_TIMEOUT: Duration = Duration::from_secs(12);
const AUDIO_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(3);
const FINAL_SILENCE_FRAMES: usize = 3; // 600 ms, using 200 ms TARGET_AUDIO_CHUNK_BYTES frames.
const SECOND_PASS_END_WINDOW_MS: u32 = 500;
const SECOND_PASS_FORCE_TO_SPEECH_MS: u32 = 1_000;

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

#[derive(Debug, thiserror::Error)]
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
    #[error("decode failed: {0}")]
    DecodeFailed(String),
}

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;
type WsSink = futures_util::stream::SplitSink<WsStream, Message>;
type SharedWriter = Arc<AsyncMutex<Option<WsSink>>>;
type PartialTranscriptCallback = Arc<dyn Fn(String) + Send + Sync>;

#[derive(Clone, Debug, PartialEq, Eq)]
struct TranscriptSegment {
    start_ms: i64,
    end_ms: Option<i64>,
    text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct TranscriptCandidate {
    text: String,
    timed_segments: Vec<TranscriptSegment>,
}

/// Sync state shared across the receive loop, the public API, and the
/// audio-consumer fast path.
#[derive(Default)]
struct SyncState {
    pending_audio: Vec<u8>,
    next_sequence: i32,
    bytes_sent: usize,
    frames_sent: usize,
    is_connected: bool,
    final_tx: Option<oneshot::Sender<Result<RawTranscript, VolcengineASRError>>>,
    runtime: Option<Handle>,
    start: Option<Instant>,
    last_audio_enqueue: Option<Instant>,
    finishing: bool,
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
}

pub struct VolcengineStreamingASR {
    credentials: VolcengineCredentials,
    hotwords: Vec<DictionaryHotword>,
    state: ParkingMutex<SyncState>,
    partial_callback: ParkingMutex<Option<PartialTranscriptCallback>>,
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
    send_done: Arc<Notify>,
}

impl VolcengineStreamingASR {
    pub fn new(credentials: VolcengineCredentials, hotwords: Vec<DictionaryHotword>) -> Self {
        Self {
            credentials,
            hotwords,
            state: ParkingMutex::new(SyncState::default()),
            partial_callback: ParkingMutex::new(None),
            writer: Arc::new(AsyncMutex::new(None)),
            final_rx: ParkingMutex::new(None),
            audio_tx: ParkingMutex::new(None),
            pending_sends: Arc::new(AtomicUsize::new(0)),
            send_done: Arc::new(Notify::new()),
        }
    }

    pub fn is_connected(&self) -> bool {
        self.state.lock().is_connected
    }

    pub fn set_partial_transcript_callback(
        &self,
        callback: Option<Arc<dyn Fn(String) + Send + Sync>>,
    ) {
        *self.partial_callback.lock() = callback;
    }

    fn emit_partial_transcript(&self, text: &str) {
        let callback = self.partial_callback.lock().clone();
        if let Some(callback) = callback {
            callback(text.to_string());
        }
    }

    pub async fn open_session(self: &Arc<Self>) -> Result<(), VolcengineASRError> {
        if self.credentials.app_id.is_empty()
            || self.credentials.access_token.is_empty()
            || self.credentials.resource_id.is_empty()
        {
            return Err(VolcengineASRError::CredentialsMissing);
        }

        let connect_id = Uuid::new_v4().to_string();
        let mut request = ENDPOINT
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
            st.is_connected = true;
            st.final_tx = Some(tx);
            st.runtime = Some(Handle::current());
            st.start = Some(Instant::now());
            st.last_audio_enqueue = Some(Instant::now());
            st.finishing = false;
            st.last_partial_text.clear();
            st.best_transcript_text.clear();
            st.best_transcript_segments.clear();
        }
        self.pending_sends.store(0, Ordering::SeqCst);
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
        tokio::spawn(async move {
            while let Some((seq, chunk)) = audio_rx.recv().await {
                let frame = frame::build(
                    MessageType::AudioOnlyRequest,
                    Flags::PositiveSequence,
                    Serialization::None,
                    &chunk,
                    Some(seq),
                );
                if let Err(e) = send_binary(&writer_for_worker, frame).await {
                    log::error!("[asr] audio frame seq={} send 失败: {}", seq, e);
                }
                if pending_for_worker.fetch_sub(1, Ordering::SeqCst) == 1 {
                    notify_for_worker.notify_waiters();
                }
            }
        });
        let keepalive_tx = self.audio_tx.lock().as_ref().cloned();
        if let Some(keepalive_tx) = keepalive_tx {
            let weak_for_keepalive = Arc::downgrade(self);
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(AUDIO_KEEPALIVE_INTERVAL).await;
                    let Some(this) = weak_for_keepalive.upgrade() else {
                        break;
                    };
                    let seq = {
                        let mut st = this.state.lock();
                        if !st.is_connected || st.finishing {
                            break;
                        }
                        if st
                            .last_audio_enqueue
                            .is_some_and(|last| last.elapsed() < AUDIO_KEEPALIVE_INTERVAL)
                        {
                            continue;
                        }
                        let seq = st.next_sequence;
                        st.next_sequence += 1;
                        st.bytes_sent += TARGET_AUDIO_CHUNK_BYTES;
                        st.frames_sent += 1;
                        st.last_audio_enqueue = Some(Instant::now());
                        seq
                    };
                    this.pending_sends.fetch_add(1, Ordering::SeqCst);
                    if keepalive_tx
                        .send((seq, vec![0; TARGET_AUDIO_CHUNK_BYTES]))
                        .is_err()
                    {
                        if this.pending_sends.fetch_sub(1, Ordering::SeqCst) == 1 {
                            this.send_done.notify_waiters();
                        }
                        break;
                    }
                    log::debug!("[asr] sent silence keepalive frame seq={seq}");
                }
            });
        }

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
        self.state.lock().finishing = true;
        // 等所有 fire-and-forget 发送完成。否则末帧（NegativeSequence）可能比尾部
        // chunk 先到服务端，被识别为「流已结束」之后再到的 chunk 全部丢弃 = 尾句吞掉。
        // 给一个 800ms 上限避免极端网络下永远等。
        let drain_deadline = Instant::now() + std::time::Duration::from_millis(800);
        while self.pending_sends.load(Ordering::SeqCst) > 0 {
            let remaining = drain_deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                log::warn!(
                    "[asr] send_last_frame: pending {} 帧未发送完，超时强制继续",
                    self.pending_sends.load(Ordering::SeqCst)
                );
                break;
            }
            // notified() 返回 future，被 timeout 包住 → 等待发送完成或超时
            let _ = tokio::time::timeout(remaining, self.send_done.notified()).await;
        }

        // Drain leftover audio (if any) into one final positive-sequence frame.
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
            send_binary(&self.writer, frame).await?;
        }

        for _ in 0..FINAL_SILENCE_FRAMES {
            let seq = self.allocate_positive_seq();
            let frame = frame::build(
                MessageType::AudioOnlyRequest,
                Flags::PositiveSequence,
                Serialization::None,
                &vec![0; TARGET_AUDIO_CHUNK_BYTES],
                Some(seq),
            );
            {
                let mut st = self.state.lock();
                st.bytes_sent += TARGET_AUDIO_CHUNK_BYTES;
                st.frames_sent += 1;
            }
            send_binary(&self.writer, frame).await?;
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
        send_binary(&self.writer, frame).await?;

        let (total_bytes, total_frames) = {
            let st = self.state.lock();
            (st.bytes_sent, st.frames_sent)
        };
        let duration_ms = (total_bytes as f64 / BYTES_PER_MS) as u64;
        log::info!(
            "[asr] 发送总结：{} audio frames, {} bytes (~{} ms)",
            total_frames,
            total_bytes,
            duration_ms
        );
        Ok(())
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
        let Some(rx) = rx else {
            return Err(VolcengineASRError::NoFinalResult);
        };
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(VolcengineASRError::NoFinalResult),
            Err(_) => {
                log::error!(
                    "[asr] final result timed out after {} ms",
                    timeout.as_millis()
                );
                self.cancel();
                Err(VolcengineASRError::FinalResultTimeout)
            }
        }
    }

    pub fn cancel(&self) {
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
        self.signal_error(VolcengineASRError::NoFinalResult);
    }

    // ---- internals ----

    fn build_first_frame_payload(&self, connect_id: &str) -> Value {
        let mut request = json!({
            "model_name": "bigmodel",
            "enable_nonstream": true,
            "enable_itn": true,
            "enable_punc": true,
            "show_utterances": true,
            "result_type": "full",
            "end_window_size": SECOND_PASS_END_WINDOW_MS,
            "force_to_speech_time": SECOND_PASS_FORCE_TO_SPEECH_MS,
        });
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

        if let Ok(payload_str) = std::str::from_utf8(&parsed.payload) {
            log::info!(
                "[asr] server JSON: {}",
                payload_str.chars().take(400).collect::<String>()
            );
        }

        let json: Value = match serde_json::from_slice(&parsed.payload) {
            Ok(v) => v,
            Err(_) => return true,
        };
        let Some(result) = normalized_result(&json) else {
            return true;
        };

        // 流结束信号只信帧头 flags（lastPacket / negativeSequence）。
        // 之前误把 utterance.definite=true 当成流结束——但那只代表"这一段语音已固化"，
        // 用户可能还在继续说。结果一收到第一个 definite=true 就关掉接收，
        // 后面用户讲的内容全部丢失（实测丢了 9 秒）。
        let has_final = parsed.is_final();
        let candidate = transcript_candidate_from_result(result);
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
            let (merged, segments) = merge_streaming_candidate(
                &state.best_transcript_text,
                &state.best_transcript_segments,
                candidate,
            );
            let changed = !merged.is_empty() && state.last_partial_text != merged;
            if !merged.is_empty() {
                state.best_transcript_text = merged.clone();
                state.best_transcript_segments = segments;
                state.last_partial_text = merged.clone();
            }
            (merged, changed)
        };

        // 缓存最新的 transcript：服务端在 final 帧前断连时 fallback 用，
        // 同时把稳定预览推给胶囊。final 也要推一次，因为火山 two-pass
        // 经常在 final 才补齐长句前半段；这能让胶囊消失前先显示完整预览。
        if (partial_changed || has_final) && !full_text.is_empty() {
            self.emit_partial_transcript(&full_text);
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
        let tx = self.state.lock().final_tx.take();
        if let Some(tx) = tx {
            let _ = tx.send(Err(err));
        }
    }

    /// 服务端 close / 网络中断时调用：如果有缓存的 partial 文本，作为 transcript
    /// 兜底返回；否则才报错。配合 `last_partial_text` 实现「至少不丢用户已识别出的话」。
    fn fallback_to_partial_or_error(&self, err: VolcengineASRError) {
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
            if !out.is_empty() {
                st.last_audio_enqueue = Some(Instant::now());
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
            self.pending_sends.fetch_add(1, Ordering::SeqCst);
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

async fn send_binary(writer: &SharedWriter, data: Vec<u8>) -> Result<(), VolcengineASRError> {
    let mut guard = writer.lock().await;
    let Some(sink) = guard.as_mut() else {
        return Err(VolcengineASRError::ConnectionFailed(
            "websocket not open".into(),
        ));
    };
    sink.send(Message::Binary(data))
        .await
        .map_err(|e| VolcengineASRError::ConnectionFailed(e.to_string()))
}

fn hex_prefix(data: &[u8], n: usize) -> String {
    data.iter()
        .take(n)
        .map(|b| format!("{:02x}", b))
        .collect::<Vec<_>>()
        .join("")
}

fn normalized_result(json: &Value) -> Option<&Value> {
    if let Some(obj) = json.get("result") {
        if obj.is_object() {
            return Some(obj);
        }
        if let Some(arr) = obj.as_array() {
            if let Some(first) = arr.first() {
                return Some(first);
            }
        }
    }
    if json.get("text").and_then(|v| v.as_str()).is_some() {
        return Some(json);
    }
    None
}

fn transcript_text_from_result(result: &Value) -> String {
    transcript_candidate_from_result(result).text
}

fn transcript_candidate_from_result(result: &Value) -> TranscriptCandidate {
    let result_text = result.get("text").and_then(|v| v.as_str()).unwrap_or("");
    let (utterance_text, timed_segments) =
        if let Some(utterances) = result.get("utterances").and_then(|v| v.as_array()) {
            let mut pieces: Vec<&str> = Vec::new();
            let mut timed_segments = Vec::new();
            for utterance in utterances {
                if let Some(text) = utterance.get("text").and_then(|t| t.as_str()) {
                    pieces.push(text);
                }
                if let Some(segment) = transcript_segment_from_utterance(utterance) {
                    timed_segments.push(segment);
                }
            }
            (pieces.join(""), timed_segments)
        } else {
            (String::new(), Vec::new())
        };

    TranscriptCandidate {
        text: choose_transcript_text(result_text, &utterance_text),
        timed_segments,
    }
}

fn transcript_segment_from_utterance(utterance: &Value) -> Option<TranscriptSegment> {
    let text = utterance.get("text").and_then(|t| t.as_str())?.trim();
    if text.is_empty() {
        return None;
    }
    let start_ms = first_word_millis_field(utterance, &["start_time", "startTime", "start_ms"])
        .or_else(|| value_millis_field(utterance, &["start_time", "startTime", "start_ms"]))?;
    let end_ms = last_word_millis_field(utterance, &["end_time", "endTime", "end_ms"])
        .or_else(|| value_millis_field(utterance, &["end_time", "endTime", "end_ms"]));
    Some(TranscriptSegment {
        start_ms,
        end_ms,
        text: text.to_string(),
    })
}

fn first_word_millis_field(value: &Value, keys: &[&str]) -> Option<i64> {
    value
        .get("words")
        .and_then(|words| words.as_array())
        .and_then(|words| words.iter().find_map(|word| value_millis_field(word, keys)))
}

fn last_word_millis_field(value: &Value, keys: &[&str]) -> Option<i64> {
    value
        .get("words")
        .and_then(|words| words.as_array())
        .and_then(|words| {
            words
                .iter()
                .rev()
                .find_map(|word| value_millis_field(word, keys))
        })
}

fn value_millis_field(value: &Value, keys: &[&str]) -> Option<i64> {
    keys.iter()
        .filter_map(|key| value.get(*key))
        .find_map(value_as_i64)
}

fn value_as_i64(value: &Value) -> Option<i64> {
    if let Some(n) = value.as_i64() {
        return Some(n);
    }
    if let Some(n) = value.as_u64() {
        return i64::try_from(n).ok();
    }
    if let Some(n) = value.as_f64() {
        if n.is_finite() {
            return Some(n.round() as i64);
        }
    }
    value.as_str()?.trim().parse::<i64>().ok()
}

fn choose_transcript_text(result_text: &str, utterance_text: &str) -> String {
    let result_text = result_text.trim();
    let utterance_text = utterance_text.trim();
    if result_text.is_empty() {
        return utterance_text.to_string();
    }
    if utterance_text.is_empty() {
        return result_text.to_string();
    }
    if has_duplicate_prefix_before_suffix(result_text, utterance_text) {
        return utterance_text.to_string();
    }
    if has_duplicate_prefix_before_suffix(utterance_text, result_text) {
        return result_text.to_string();
    }
    if has_duplicate_tail_after_full_revision(utterance_text, result_text) {
        return result_text.to_string();
    }
    if has_duplicate_tail_after_full_revision(result_text, utterance_text) {
        return utterance_text.to_string();
    }
    if result_text.contains(utterance_text) {
        return result_text.to_string();
    }
    if utterance_text.contains(result_text) {
        return utterance_text.to_string();
    }
    if utterance_text.chars().count() > result_text.chars().count() {
        utterance_text.to_string()
    } else {
        result_text.to_string()
    }
}

fn merge_streaming_transcript(previous: &str, current: &str) -> String {
    let previous = previous.trim();
    let current = current.trim();
    if previous.is_empty() {
        return current.to_string();
    }
    if current.is_empty() {
        return previous.to_string();
    }
    if has_duplicate_prefix_before_suffix(current, previous) {
        return previous.to_string();
    }
    if current.contains(previous) {
        return current.to_string();
    }
    if previous.contains(current) {
        return previous.to_string();
    }
    if compact_transcript_contains_window(previous, current) {
        return previous.to_string();
    }
    if compact_transcript_contains_window(current, previous) {
        return current.to_string();
    }

    if is_same_prefix_streaming_revision(previous, current) {
        return current.to_string();
    }

    let previous_chars: Vec<char> = previous.chars().collect();
    let current_chars: Vec<char> = current.chars().collect();
    let max_overlap = previous_chars.len().min(current_chars.len());
    for overlap in (2..=max_overlap).rev() {
        if previous_chars[previous_chars.len() - overlap..] == current_chars[..overlap] {
            let suffix: String = current_chars[overlap..].iter().collect();
            return format!("{previous}{suffix}");
        }
    }

    if current_chars.len() < 4 {
        return previous.to_string();
    }

    join_transcript_segments(previous, current)
}

fn has_duplicate_prefix_before_suffix(candidate: &str, stable_suffix: &str) -> bool {
    let Some(leading) = candidate.strip_suffix(stable_suffix) else {
        return false;
    };
    is_duplicate_transcript_prefix(stable_suffix, leading)
}

fn is_duplicate_transcript_prefix(full_text: &str, prefix: &str) -> bool {
    let full = compact_transcript_for_duplicate_check(full_text);
    let prefix = compact_transcript_for_duplicate_check(prefix);
    prefix.chars().count() >= 4 && full.starts_with(&prefix)
}

fn has_duplicate_tail_after_full_revision(candidate: &str, stable_full: &str) -> bool {
    const MIN_DUPLICATE_TAIL_CHARS: usize = 8;
    const MAX_DUPLICATE_TAIL_CER: f64 = 0.18;

    let candidate = compact_transcript_for_duplicate_check(candidate);
    let stable_full = compact_transcript_for_duplicate_check(stable_full);
    if candidate == stable_full || !candidate.starts_with(&stable_full) {
        return false;
    }

    let stable_len = stable_full.chars().count();
    let duplicate_tail: String = candidate.chars().skip(stable_len).collect();
    let duplicate_len = duplicate_tail.chars().count();
    if duplicate_len < MIN_DUPLICATE_TAIL_CHARS || duplicate_len > stable_len {
        return false;
    }

    let stable_suffix: String = stable_full
        .chars()
        .skip(stable_len - duplicate_len)
        .collect();
    let distance = char_edit_distance(&stable_suffix, &duplicate_tail);
    (distance as f64 / duplicate_len as f64) <= MAX_DUPLICATE_TAIL_CER
}

fn char_edit_distance(left: &str, right: &str) -> usize {
    let left: Vec<char> = left.chars().collect();
    let right: Vec<char> = right.chars().collect();
    if left.is_empty() {
        return right.len();
    }
    if right.is_empty() {
        return left.len();
    }

    let mut previous: Vec<usize> = (0..=right.len()).collect();
    let mut current = vec![0; right.len() + 1];
    for (i, left_ch) in left.iter().enumerate() {
        current[0] = i + 1;
        for (j, right_ch) in right.iter().enumerate() {
            let substitution_cost = usize::from(left_ch != right_ch);
            current[j + 1] = (previous[j + 1] + 1)
                .min(current[j] + 1)
                .min(previous[j] + substitution_cost);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[right.len()]
}

fn compact_transcript_for_duplicate_check(text: &str) -> String {
    text.chars()
        .filter(|ch| {
            !ch.is_whitespace()
                && !matches!(
                    ch,
                    '，' | '。'
                        | '、'
                        | '；'
                        | '：'
                        | '！'
                        | '？'
                        | ','
                        | '.'
                        | ';'
                        | ':'
                        | '!'
                        | '?'
                )
        })
        .collect()
}

fn compact_transcript_contains_window(container: &str, window: &str) -> bool {
    const MIN_EXACT_WINDOW_CHARS: usize = 6;
    const MIN_FUZZY_WINDOW_CHARS: usize = 12;
    const MAX_FUZZY_WINDOW_CER: f64 = 0.12;

    let container = compact_transcript_for_duplicate_check(container);
    let window = compact_transcript_for_duplicate_check(window);
    let window_len = window.chars().count();
    if window_len < MIN_EXACT_WINDOW_CHARS {
        return false;
    }
    if container.contains(&window) {
        return true;
    }

    let container_chars: Vec<char> = container.chars().collect();
    if window_len < MIN_FUZZY_WINDOW_CHARS || container_chars.len() < window_len {
        return false;
    }

    let max_distance = ((window_len as f64) * MAX_FUZZY_WINDOW_CER).floor() as usize;
    if max_distance == 0 {
        return false;
    }
    for start in 0..=container_chars.len() - window_len {
        let candidate: String = container_chars[start..start + window_len].iter().collect();
        if char_edit_distance(&candidate, &window) <= max_distance {
            return true;
        }
    }
    false
}

fn is_unstable_initial_partial(previous: &str, current: &str) -> bool {
    if !previous.trim().is_empty() {
        return false;
    }

    let compact = compact_transcript_for_duplicate_check(current);
    let char_count = compact.chars().count();
    if char_count == 0 {
        return true;
    }
    char_count <= 3
}

fn is_same_prefix_streaming_revision(previous: &str, current: &str) -> bool {
    const MIN_COMMON_PREFIX_CHARS: usize = 10;
    let previous_compact = compact_transcript_for_duplicate_check(previous);
    let current_compact = compact_transcript_for_duplicate_check(current);
    let previous_len = previous_compact.chars().count();
    let current_len = current_compact.chars().count();
    if current_len > previous_len && current_compact.starts_with(&previous_compact) {
        return true;
    }
    if previous_len < MIN_COMMON_PREFIX_CHARS || current_len < MIN_COMMON_PREFIX_CHARS {
        return false;
    }

    let common_prefix = common_prefix_char_count(&previous_compact, &current_compact);
    if common_prefix < MIN_COMMON_PREFIX_CHARS {
        return false;
    }

    if current_len + 4 >= previous_len {
        return true;
    }

    repeated_prefix_occurs(&previous_compact, &current_compact, MIN_COMMON_PREFIX_CHARS)
}

fn common_prefix_char_count(left: &str, right: &str) -> usize {
    left.chars()
        .zip(right.chars())
        .take_while(|(left, right)| left == right)
        .count()
}

fn repeated_prefix_occurs(text: &str, source: &str, prefix_chars: usize) -> bool {
    let prefix: String = source.chars().take(prefix_chars).collect();
    if prefix.is_empty() {
        return false;
    }
    text.match_indices(&prefix).nth(1).is_some()
}

fn merge_streaming_candidate(
    previous_text: &str,
    previous_segments: &[TranscriptSegment],
    candidate: TranscriptCandidate,
) -> (String, Vec<TranscriptSegment>) {
    let candidate_text = candidate.text.trim().to_string();
    if candidate.timed_segments.is_empty() {
        return (
            merge_streaming_transcript(previous_text, &candidate_text),
            previous_segments.to_vec(),
        );
    }

    let previous_text = previous_text.trim();
    let mut incoming_segments = candidate.timed_segments;
    incoming_segments.sort_by_key(|segment| segment.start_ms);
    let old_segment_text = join_timed_segment_text(previous_segments);
    if let Some((merged_text, segments)) = merge_provider_segmentation_revision(
        previous_text,
        previous_segments,
        &incoming_segments,
        &candidate_text,
        &old_segment_text,
    ) {
        return (merged_text, segments);
    }

    let mut merged_segments = previous_segments.to_vec();
    let mut replaced_existing = false;
    for segment in incoming_segments {
        if let Some(index) = find_matching_timed_segment(&merged_segments, &segment) {
            replaced_existing = true;
            merged_segments[index] = merge_timed_segment(&merged_segments[index], &segment);
        } else {
            merged_segments.push(segment);
        }
    }
    merged_segments.sort_by_key(|segment| segment.start_ms);

    let new_segment_text = join_timed_segment_text(&merged_segments);
    if previous_text.is_empty() {
        return (
            choose_transcript_text(&candidate_text, &new_segment_text),
            merged_segments,
        );
    }

    if !old_segment_text.is_empty() {
        if let Some(index) = previous_text.find(&old_segment_text) {
            let mut merged_text = String::with_capacity(
                previous_text.len() - old_segment_text.len() + new_segment_text.len(),
            );
            merged_text.push_str(&previous_text[..index]);
            merged_text.push_str(&new_segment_text);
            merged_text.push_str(&previous_text[index + old_segment_text.len()..]);
            return (
                choose_transcript_text(&candidate_text, &merged_text),
                merged_segments,
            );
        }
    }

    if replaced_existing {
        return (
            choose_transcript_text(previous_text, &new_segment_text),
            merged_segments,
        );
    }

    let append_text = choose_transcript_text(&candidate_text, &new_segment_text);
    (
        merge_streaming_transcript(previous_text, &append_text),
        merged_segments,
    )
}

fn merge_provider_segmentation_revision(
    previous_text: &str,
    previous_segments: &[TranscriptSegment],
    incoming_segments: &[TranscriptSegment],
    candidate_text: &str,
    old_segment_text: &str,
) -> Option<(String, Vec<TranscriptSegment>)> {
    if previous_text.is_empty()
        || incoming_segments.len() <= 1
        || candidate_text.chars().count() + 4 < previous_text.chars().count()
    {
        return None;
    }

    let splits_existing_segment = incoming_segments.iter().any(|incoming| {
        find_matching_timed_segment(previous_segments, incoming)
            .and_then(|index| previous_segments.get(index))
            .is_some_and(|existing| {
                existing.text.starts_with(&incoming.text)
                    && existing.text.chars().count() > incoming.text.chars().count() + 4
            })
    });
    if !splits_existing_segment {
        return None;
    }

    let new_segment_text = join_timed_segment_text(incoming_segments);
    let merged_text = if !old_segment_text.is_empty() {
        if let Some(index) = previous_text.find(old_segment_text) {
            let mut merged = String::with_capacity(
                previous_text.len() - old_segment_text.len() + new_segment_text.len(),
            );
            merged.push_str(&previous_text[..index]);
            merged.push_str(&new_segment_text);
            merged.push_str(&previous_text[index + old_segment_text.len()..]);
            merged
        } else {
            merge_streaming_transcript(previous_text, candidate_text)
        }
    } else {
        merge_streaming_transcript(previous_text, candidate_text)
    };

    Some((
        choose_transcript_text(candidate_text, &merged_text),
        incoming_segments.to_vec(),
    ))
}

fn find_matching_timed_segment(
    segments: &[TranscriptSegment],
    incoming: &TranscriptSegment,
) -> Option<usize> {
    const START_TIME_TOLERANCE_MS: i64 = 240;
    segments
        .iter()
        .position(|segment| (segment.start_ms - incoming.start_ms).abs() <= START_TIME_TOLERANCE_MS)
}

fn merge_timed_segment(
    existing: &TranscriptSegment,
    incoming: &TranscriptSegment,
) -> TranscriptSegment {
    let text = choose_transcript_text(&existing.text, &incoming.text);
    TranscriptSegment {
        start_ms: existing.start_ms.min(incoming.start_ms),
        end_ms: match (existing.end_ms, incoming.end_ms) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        },
        text,
    }
}

fn join_timed_segment_text(segments: &[TranscriptSegment]) -> String {
    segments.iter().fold(String::new(), |merged, segment| {
        merge_streaming_transcript(&merged, &segment.text)
    })
}

fn join_transcript_segments(previous: &str, current: &str) -> String {
    let needs_space = previous
        .chars()
        .next_back()
        .is_some_and(|ch| ch.is_ascii_alphanumeric())
        && current
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_alphanumeric());
    if needs_space {
        format!("{previous} {current}")
    } else {
        format!("{previous}{current}")
    }
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
    fn first_frame_payload_enables_official_second_pass() {
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
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
    fn transcript_text_prefers_accumulated_result_when_utterances_are_suffix() {
        let result = json!({
            "text": "先导出数据。再导入数据库。最后检查汇总结果。",
            "utterances": [
                { "text": "最后检查汇总结果。" }
            ]
        });

        assert_eq!(
            transcript_text_from_result(&result),
            "先导出数据。再导入数据库。最后检查汇总结果。"
        );
    }

    #[test]
    fn transcript_text_uses_utterances_when_result_text_is_empty() {
        let result = json!({
            "text": "",
            "utterances": [
                { "text": "蓝牙音频" },
                { "text": "正在识别。" }
            ]
        });

        assert_eq!(transcript_text_from_result(&result), "蓝牙音频正在识别。");
    }

    #[test]
    fn transcript_text_uses_longer_utterances_when_they_have_full_context() {
        let result = json!({
            "text": "蓝牙音频",
            "utterances": [
                { "text": "蓝牙音频" },
                { "text": "正在发送到火山识别。" }
            ]
        });

        assert_eq!(
            transcript_text_from_result(&result),
            "蓝牙音频正在发送到火山识别。"
        );
    }

    #[test]
    fn transcript_candidate_parses_timed_utterances() {
        let result = json!({
            "text": "",
            "utterances": [
                { "text": "第一句。", "start_time": 0, "end_time": 900 },
                { "text": "第二句。", "start_time": "1200", "end_time": "2100" }
            ]
        });

        let candidate = transcript_candidate_from_result(&result);

        assert_eq!(candidate.text, "第一句。第二句。");
        assert_eq!(
            candidate.timed_segments,
            vec![
                TranscriptSegment {
                    start_ms: 0,
                    end_ms: Some(900),
                    text: "第一句。".into()
                },
                TranscriptSegment {
                    start_ms: 1200,
                    end_ms: Some(2100),
                    text: "第二句。".into()
                }
            ]
        );
    }

    #[test]
    fn transcript_candidate_prefers_word_timing_when_utterance_start_is_stale() {
        let result = json!({
            "text": "需要分开判断，测试报告里要记录蓝牙包数。",
            "utterances": [{
                "text": "需要分开判断，测试报告里要记录蓝牙包数。",
                "start_time": 1452,
                "end_time": 10402,
                "words": [
                    { "text": "需要", "start_time": 6372, "end_time": 6452 },
                    { "text": "包数", "start_time": 10100, "end_time": 10402 }
                ]
            }]
        });

        let candidate = transcript_candidate_from_result(&result);

        assert_eq!(candidate.timed_segments[0].start_ms, 6372);
        assert_eq!(candidate.timed_segments[0].end_ms, Some(10402));
    }

    #[test]
    fn merge_streaming_transcript_keeps_prefix_after_provider_reset() {
        assert_eq!(
            merge_streaming_transcript(
                "关闭所有打开的窗口，然后重启电脑，帮我查一下从北京到上海",
                "帮我查一下从北京到上海的高铁票最早的几班",
            ),
            "关闭所有打开的窗口，然后重启电脑，帮我查一下从北京到上海的高铁票最早的几班"
        );
    }

    #[test]
    fn merge_streaming_transcript_appends_disjoint_continuation_segments() {
        assert_eq!(
            merge_streaming_transcript(
                "前端胶囊只显示",
                "不能承载整段文字，最后把异常现象整理成清晰的结论。"
            ),
            "前端胶囊只显示不能承载整段文字，最后把异常现象整理成清晰的结论。"
        );
        assert_eq!(
            merge_streaming_transcript("已经包含后续内容", "后续"),
            "已经包含后续内容"
        );
        assert_eq!(
            merge_streaming_transcript("已经有稳定正文", "哎呀"),
            "已经有稳定正文"
        );
    }

    #[test]
    fn merge_streaming_transcript_ignores_restarted_middle_window() {
        let stable = "这段录音正在验证长时间语音输入，如果某一步失败就先修最基础的链路，再继续往后跑。后端需要把最终结果稳定地交给系统输入链路，最后把异常现象整理成清晰的结论。";
        let restarted_window = "如果某一步失败，就先修最基础的链路，再继续往后跑，后端需要把最终结果稳定的交给系统输入链路，最后把。";

        assert_eq!(merge_streaming_transcript(stable, restarted_window), stable);
    }

    #[test]
    fn merge_streaming_transcript_replaces_same_prefix_revision() {
        assert_eq!(
            merge_streaming_transcript(
                "现在开始进行一段完整的产品链路，长亭",
                "现在开始进行一段完整的产品链路，长听写",
            ),
            "现在开始进行一段完整的产品链路，长听写"
        );
    }

    #[test]
    fn merge_streaming_transcript_replaces_short_prefix_revision() {
        assert_eq!(
            merge_streaming_transcript("请。", "请把这段长录音完整转换成文字。"),
            "请把这段长录音完整转换成文字。"
        );
    }

    #[test]
    fn unstable_initial_partial_filters_short_interference_only_before_context() {
        assert!(is_unstable_initial_partial("", "哎呀"));
        assert!(is_unstable_initial_partial("", "嗯"));
        assert!(!is_unstable_initial_partial("", "蓝牙听写测试现在开始"));
        assert!(!is_unstable_initial_partial("已经有正文", "哎呀"));
    }

    #[test]
    fn merge_streaming_transcript_replaces_duplicate_with_clean_revision() {
        assert_eq!(
            merge_streaming_transcript(
                "现在开始进行一段完整的产品链路，长亭现在开始进行一段完整的产品链路，长听写现在开始进行一段完整的产品链路长听写测试报告",
                "现在开始进行一段完整的产品链路长听写，测试报告里要记录蓝牙包数和识别准确率。",
            ),
            "现在开始进行一段完整的产品链路长听写，测试报告里要记录蓝牙包数和识别准确率。"
        );
    }

    #[test]
    fn merge_streaming_transcript_ignores_duplicate_prefix_before_stable_full_text() {
        let stable = "我正在复盘上午的调试过程和后续安排，如果声音偏小，也要尽量保持句子结构清楚，测试报告里要记录蓝牙高速和识别准确率，最后继续执行下一项自动化回归。";
        let duplicate_prefix = "我正在复盘上午的调试过程和后续安排如果声音偏小也要尽量保持句子";

        assert_eq!(
            merge_streaming_transcript(stable, &format!("{duplicate_prefix}{stable}")),
            stable
        );
        assert_eq!(
            choose_transcript_text(&format!("{duplicate_prefix}{stable}"), stable),
            stable
        );
    }

    #[test]
    fn choose_transcript_text_prefers_final_over_duplicate_tail_revision() {
        let final_text = "现在开始进行一段完整的产品链路长听写，在观察识别完成后文字是否立即进入当前光标，遇到语速变快时需要重点关注开头和结尾是否丢失，最后把异常现象整理成清晰的结论。";
        let duplicate_tail = "再观察识别完成后文字是否立即进入当前光标，遇到语速变快时，需要重点关注开头和结尾是否丢失，最后把异常现象整理成清晰的结论";
        let duplicated = format!("{final_text}{duplicate_tail}");

        assert_eq!(choose_transcript_text(final_text, &duplicated), final_text);
    }

    #[test]
    fn merge_streaming_candidate_prefers_final_full_text_over_old_tail_segment() {
        let previous_tail = "再观察识别完成后文字是否立即进入当前光标，遇到语速变快时，需要重点关注开头和结尾是否丢失，最后把异常现象整理成清晰的结论";
        let final_text = "现在开始进行一段完整的产品链路长听写，在观察识别完成后文字是否立即进入当前光标，遇到语速变快时需要重点关注开头和结尾是否丢失，最后把异常现象整理成清晰的结论。";
        let previous_segments = vec![TranscriptSegment {
            start_ms: 4840,
            end_ms: Some(14600),
            text: previous_tail.into(),
        }];
        let candidate = TranscriptCandidate {
            text: final_text.into(),
            timed_segments: vec![TranscriptSegment {
                start_ms: 720,
                end_ms: Some(15132),
                text: final_text.into(),
            }],
        };

        let (merged, _segments) =
            merge_streaming_candidate(previous_tail, &previous_segments, candidate);

        assert_eq!(merged, final_text);
    }

    #[test]
    fn merge_streaming_candidate_replaces_repeated_timed_suffix_instead_of_appending() {
        let previous_segments = vec![TranscriptSegment {
            start_ms: 7602,
            end_ms: Some(10300),
            text: "看到稳定的预览内容，同时检查历史记录里是否保存了完整的最终文本。最后".into(),
        }];
        let candidate = TranscriptCandidate {
            text: "看到稳定的预览内容，同时检查历史记录里是否保存了完整的最终文本，最后继续执行下一项自动化回归。".into(),
            timed_segments: vec![TranscriptSegment {
                start_ms: 7602,
                end_ms: Some(14500),
                text: "看到稳定的预览内容，同时检查历史记录里是否保存了完整的最终文本，最后继续执行下一项自动化回归。".into(),
            }],
        };

        let (merged, segments) = merge_streaming_candidate(
            "看到稳定的预览内容，同时检查历史记录里是否保存了完整的最终文本。最后",
            &previous_segments,
            candidate,
        );

        assert_eq!(
            merged,
            "看到稳定的预览内容，同时检查历史记录里是否保存了完整的最终文本，最后继续执行下一项自动化回归。"
        );
        assert_eq!(segments.len(), 1);
    }

    #[test]
    fn merge_streaming_candidate_preserves_prefix_when_timed_suffix_grows() {
        let previous_text = "这段长录音用来验证真实会议记录的输入体验，先确认胶囊里可以实时看到稳定的预览内容，同时检查历史记录。";
        let old_suffix = "看到稳定的预览内容，同时检查历史记录。";
        let previous_segments = vec![TranscriptSegment {
            start_ms: 6200,
            end_ms: Some(9000),
            text: old_suffix.into(),
        }];
        let candidate = TranscriptCandidate {
            text: "看到稳定的预览内容，同时检查历史记录里是否保存了完整的最终文本。".into(),
            timed_segments: vec![TranscriptSegment {
                start_ms: 6200,
                end_ms: Some(11400),
                text: "看到稳定的预览内容，同时检查历史记录里是否保存了完整的最终文本。".into(),
            }],
        };

        let (merged, _segments) =
            merge_streaming_candidate(previous_text, &previous_segments, candidate);

        assert_eq!(
            merged,
            "这段长录音用来验证真实会议记录的输入体验，先确认胶囊里可以实时看到稳定的预览内容，同时检查历史记录里是否保存了完整的最终文本。"
        );
    }

    #[test]
    fn merge_streaming_candidate_replaces_provider_split_without_duplication() {
        let previous_text = "我正在复盘上午的调试过程和后续安排。这类场景更接近日常口述备忘";
        let previous_segments = vec![TranscriptSegment {
            start_ms: 912,
            end_ms: Some(8172),
            text: previous_text.into(),
        }];
        let candidate = TranscriptCandidate {
            text: "我正在复盘上午的调试过程和后续安排。这类场景更接近日常口述、备忘和会议纪要"
                .into(),
            timed_segments: vec![
                TranscriptSegment {
                    start_ms: 912,
                    end_ms: Some(4192),
                    text: "我正在复盘上午的调试过程和后续安排。".into(),
                },
                TranscriptSegment {
                    start_ms: 5200,
                    end_ms: Some(9000),
                    text: "这类场景更接近日常口述、备忘和会议纪要".into(),
                },
            ],
        };

        let (merged, segments) =
            merge_streaming_candidate(previous_text, &previous_segments, candidate);

        assert_eq!(
            merged,
            "我正在复盘上午的调试过程和后续安排。这类场景更接近日常口述、备忘和会议纪要"
        );
        assert_eq!(segments.len(), 2);
    }

    #[test]
    fn merge_streaming_candidate_appends_disjoint_timed_segments_once() {
        let previous_segments = vec![TranscriptSegment {
            start_ms: 0,
            end_ms: Some(900),
            text: "第一句。".into(),
        }];
        let candidate = TranscriptCandidate {
            text: "第二句。".into(),
            timed_segments: vec![TranscriptSegment {
                start_ms: 1300,
                end_ms: Some(2100),
                text: "第二句。".into(),
            }],
        };

        let (merged, segments) =
            merge_streaming_candidate("第一句。", &previous_segments, candidate);

        assert_eq!(merged, "第一句。第二句。");
        assert_eq!(segments.len(), 2);
    }

    #[test]
    fn merge_streaming_candidate_preserves_prefix_when_provider_streams_tail_segment() {
        let previous_text =
            "现在开始进行一段完整的产品链路，长听写短句回归和长段落回归需要分开判断";
        let previous_segments = vec![TranscriptSegment {
            start_ms: 1812,
            end_ms: Some(7402),
            text: previous_text.into(),
        }];
        let candidate = TranscriptCandidate {
            text: "需要分开判断，测试报告里要记录蓝牙包数和识别准确率".into(),
            timed_segments: vec![TranscriptSegment {
                start_ms: 6372,
                end_ms: Some(10602),
                text: "需要分开判断，测试报告里要记录蓝牙包数和识别准确率".into(),
            }],
        };

        let (merged, segments) =
            merge_streaming_candidate(previous_text, &previous_segments, candidate);

        assert_eq!(
            merged,
            "现在开始进行一段完整的产品链路，长听写短句回归和长段落回归需要分开判断，测试报告里要记录蓝牙包数和识别准确率"
        );
        assert_eq!(segments.len(), 2);
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
}
