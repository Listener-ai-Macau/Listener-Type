//! Windows computer-microphone wake is a separate capture owner from BLE.
//! It lends its live recorder to the ordinary microphone dictation pipeline.
use super::microphone_wake_buffer::WakeBuffer;
use super::microphone_wake_decoder::{Chunk, Decoder};
use super::*;
use crate::recorder::{AudioConsumer, RecorderError};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::Weak;

const FEED_BYTES: usize = 5_120; // 160 ms; no per-device-callback inference.

#[derive(Clone, Debug, PartialEq, Eq)]
struct Config {
    enabled: bool,
    device: String,
    phrase: String,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Status {
    pub state: String,
    pub captured_samples: u64,
    pub wakes: u64,
    pub error: Option<String>,
}

struct IdleCapture {
    recorder: Recorder,
    errors: Receiver<RecorderError>,
    ingress: Arc<Ingress>,
    cancel: Arc<AtomicBool>,
    keyword_end: Option<u64>,
    claimed_session: Option<SessionId>,
    pending_since: Option<Instant>,
}

impl IdleCapture {
    fn stop(self) {
        self.cancel.store(true, Ordering::SeqCst);
        self.recorder.stop();
    }
}

struct Data {
    config: Option<Config>,
    generation: u64,
    capture: Option<IdleCapture>,
    wake_start_owner: Option<SessionId>,
    dictation_owner: Option<SessionId>,
    retry_at: Instant,
    status: Status,
}

pub(super) struct Runtime {
    data: Mutex<Data>,
    notify: tokio::sync::Notify,
    started: AtomicBool,
    preview_active: AtomicBool,
}

impl Default for Runtime {
    fn default() -> Self {
        Self {
            data: Mutex::new(Data {
                config: None,
                generation: 0,
                capture: None,
                wake_start_owner: None,
                dictation_owner: None,
                retry_at: Instant::now(),
                status: Status {
                    state: "off".into(),
                    captured_samples: 0,
                    wakes: 0,
                    error: None,
                },
            }),
            notify: tokio::sync::Notify::new(),
            started: AtomicBool::new(false),
            preview_active: AtomicBool::new(false),
        }
    }
}

struct Ingress {
    buffer: Mutex<WakeBuffer>,
    pending: Mutex<Vec<u8>>,
    sender: SyncSender<Chunk>,
}

struct WorkerExit {
    runtime: Arc<Runtime>,
    token: u64,
}
impl Drop for WorkerExit {
    fn drop(&mut self) {
        self.runtime.worker_exited(self.token);
    }
}

impl AudioConsumer for Ingress {
    fn consume_pcm_chunk(&self, pcm: &[u8]) {
        if pcm.len() % 2 != 0 {
            return;
        }
        let mut buffer = self.buffer.lock();
        let first_chunk = buffer.end == 0;
        buffer.append(pcm);
        if first_chunk {
            log::info!(
                "[microphone-wake] capture delivered PCM samples={}",
                pcm.len() / 2
            );
        }
        if buffer.detected {
            return;
        }
        let mut pending = self.pending.lock();
        pending.extend_from_slice(pcm);
        if pending.len() >= FEED_BYTES {
            let pcm = std::mem::take(&mut *pending);
            let end = buffer.end;
            // A slow decoder cannot block the native capture callback. Gaps
            // are recovered from the same bounded ring, never hidden as silence.
            let _ = self.sender.try_send(Chunk {
                start: end - pcm.len() as u64,
                end,
                pcm,
            });
        }
    }
}

impl Runtime {
    pub fn status(&self) -> Status {
        let data = self.data.lock();
        let mut status = data.status.clone();
        if let Some(capture) = data.capture.as_ref() {
            status.captured_samples = capture.ingress.buffer.lock().end / 2;
        }
        status
    }

    pub fn refresh(&self) {
        self.notify.notify_one();
    }

    pub fn owns_dictation_session(&self, session: SessionId) -> bool {
        self.data.lock().dictation_owner == Some(session)
    }

    #[cfg(test)]
    pub fn mark_dictation_owner_for_test(&self, session: SessionId) {
        self.data.lock().dictation_owner = Some(session);
    }

    pub fn set_preview_active(&self, active: bool) {
        self.preview_active.store(active, Ordering::SeqCst);
        if active {
            self.stop_idle();
        }
        self.refresh();
    }

    pub fn stop_idle(&self) {
        let capture = {
            let mut data = self.data.lock();
            data.generation += 1;
            data.capture.take()
        };
        if let Some(capture) = capture {
            capture.stop();
        }
    }

    fn reject_candidate(&self, token: u64) {
        let capture = {
            let mut data = self.data.lock();
            if data.generation != token {
                return;
            }
            data.generation += 1;
            data.capture.take()
        };
        if let Some(capture) = capture {
            capture.stop();
        }
        self.refresh();
    }

    /// Caller owns SessionState first. No path holds this mutex while acquiring
    /// SessionState, joining capture, or publishing a capsule.
    pub fn claim_start(
        &self,
        token: u64,
        create: impl FnOnce() -> Option<SessionId>,
    ) -> Option<SessionId> {
        let mut data = self.data.lock();
        if data.generation != token || !data.config.as_ref().is_some_and(|c| c.enabled) {
            return None;
        }
        let capture = data.capture.as_mut()?;
        if capture.keyword_end.is_none() || capture.claimed_session.is_some() {
            return None;
        }
        let session = create()?;
        capture.claimed_session = Some(session);
        data.wake_start_owner = Some(session);
        data.dictation_owner = Some(session);
        Some(session)
    }

    pub fn adopt(
        &self,
        session: SessionId,
        consumer: Arc<dyn AudioConsumer>,
        level: Arc<dyn Fn(f32) + Send + Sync>,
        archive: Option<PathBuf>,
    ) -> Option<Result<(Recorder, Receiver<RecorderError>, bool), String>> {
        let capture = {
            let mut data = self.data.lock();
            if data.wake_start_owner != Some(session) {
                return None;
            }
            data.wake_start_owner = None;
            if !data
                .capture
                .as_ref()
                .is_some_and(|c| c.claimed_session == Some(session))
            {
                return Some(Err(
                    "microphone wake capture was cancelled before handoff".into()
                ));
            }
            data.status.state = "recording".into();
            data.capture.take().expect("claimed capture")
        };
        capture.cancel.store(true, Ordering::SeqCst);
        let boundary = capture.keyword_end.expect("claimed keyword boundary");
        match capture.recorder.handoff(consumer, level, archive, || {
            capture.ingress.buffer.lock().body_after(boundary)
        }) {
            Ok(active) => Some(Ok((capture.recorder, capture.errors, active))),
            Err(error) => {
                capture.stop();
                Some(Err(error))
            }
        }
    }

    pub fn start_supervisor(self: &Arc<Self>, inner: Weak<Inner>) {
        if self.started.swap(true, Ordering::SeqCst) {
            return;
        }
        let runtime = Arc::clone(self);
        async_runtime::spawn(async move {
            loop {
                let Some(current) = inner.upgrade() else {
                    runtime.stop_idle();
                    break;
                };
                if current.shutdown.load(Ordering::SeqCst) {
                    runtime.stop_idle();
                    break;
                }
                runtime.reconcile(&current).await;
                drop(current);
                let enabled = runtime
                    .data
                    .lock()
                    .config
                    .as_ref()
                    .is_some_and(|c| c.enabled);
                if enabled {
                    tokio::select! { _ = runtime.notify.notified() => {}, _ = tokio::time::sleep(Duration::from_millis(400)) => {} }
                } else {
                    runtime.notify.notified().await;
                }
            }
        });
    }

    async fn reconcile(self: &Arc<Self>, inner: &Arc<Inner>) {
        let prefs = inner.prefs.get();
        let config = Config {
            enabled: prefs.microphone_wake_enabled
                && prefs.dictation_input_source == DictationInputSource::Microphone,
            device: prefs.microphone_device_name,
            phrase: prefs.voice_wake_phrase,
        };
        let idle = inner.state.lock().phase == SessionPhase::Idle
            && inner.qa_state.lock().phase == super::qa::QaPhase::Idle
            && !inner.embedded_ble_ota_active.load(Ordering::SeqCst)
            && !self.preview_active.load(Ordering::SeqCst);
        let mut stop = None;
        let token = {
            let mut data = self.data.lock();
            if data.config.as_ref() != Some(&config) {
                data.generation += 1;
                stop = data.capture.take();
                data.config = Some(config.clone());
                data.retry_at = Instant::now();
                data.status.error = None;
            }
            let claimed = data
                .capture
                .as_ref()
                .is_some_and(|c| c.claimed_session.is_some());
            let expired = data.capture.as_ref().is_some_and(|c| {
                c.pending_since
                    .is_some_and(|t| t.elapsed() > Duration::from_secs(8))
            });
            let capture_failed = data
                .capture
                .as_ref()
                .is_some_and(|c| !matches!(c.errors.try_recv(), Err(mpsc::TryRecvError::Empty)));
            if ((!idle || !config.enabled) && !claimed) || expired || capture_failed {
                data.generation += 1;
                if let Some(capture) = data.capture.take() {
                    stop = Some(capture);
                }
                if expired || capture_failed {
                    data.retry_at = Instant::now() + Duration::from_secs(2);
                }
            }
            if !config.enabled {
                data.status.state = "off".into();
                None
            } else if !idle {
                data.status.state = if claimed { "starting" } else { "paused" }.into();
                None
            } else if data.capture.is_some() {
                None
            } else if Instant::now() < data.retry_at {
                None
            } else {
                data.generation += 1;
                data.status.state = "preparing".into();
                Some(data.generation)
            }
        };
        if let Some(capture) = stop {
            let _ = async_runtime::spawn_blocking(move || capture.stop()).await;
        }
        let Some(token) = token else {
            return;
        };
        let config_for_start = config.clone();
        let built = async_runtime::spawn_blocking(move || {
            if crate::permissions::windows_microphone_access_explicitly_denied() {
                return Err("microphone_permission_denied".to_string());
            }
            crate::speaker_verification::prepare_runtime_assets()?;
            crate::asr::local::wake_helper::preload()?;
            let detector =
                crate::wake_phrase::StreamingDetector::new_strict(&config_for_start.phrase)?;
            let (sender, receiver) = mpsc::sync_channel(8);
            let ingress = Arc::new(Ingress {
                buffer: Mutex::new(WakeBuffer::default()),
                pending: Mutex::new(Vec::new()),
                sender,
            });
            let consumer: Arc<dyn AudioConsumer> = ingress.clone();
            let (recorder, errors, _) = Recorder::start(
                if config_for_start.device.is_empty() {
                    None
                } else {
                    Some(config_for_start.device)
                },
                consumer,
                Arc::new(|_| {}),
                None,
            )
            .map_err(|e| e.to_string())?;
            recorder.set_diagnostics_enabled(false);
            Ok((
                IdleCapture {
                    recorder,
                    errors,
                    ingress,
                    cancel: Arc::new(AtomicBool::new(false)),
                    keyword_end: None,
                    claimed_session: None,
                    pending_since: None,
                },
                detector,
                receiver,
            ))
        })
        .await;
        match built {
            Ok(Ok((capture, detector, receiver))) => {
                let ingress = Arc::clone(&capture.ingress);
                let cancel = Arc::clone(&capture.cancel);
                let accepted = {
                    let mut data = self.data.lock();
                    if data.generation == token && data.config.as_ref() == Some(&config) {
                        data.capture = Some(capture);
                        data.status.state = "listening".into();
                        data.status.error = None;
                        true
                    } else {
                        drop(data);
                        capture.stop();
                        false
                    }
                };
                if accepted {
                    log::info!("[microphone-wake] listening generation={token}");
                    let weak = Arc::downgrade(inner);
                    let runtime = Arc::clone(self);
                    if let Err(error) = std::thread::Builder::new()
                        .name("listener-microphone-wake".into())
                        .spawn(move || {
                            let _exit = WorkerExit {
                                runtime: Arc::clone(&runtime),
                                token,
                            };
                            run_detector(
                                weak,
                                runtime,
                                token,
                                config.phrase,
                                ingress,
                                cancel,
                                detector,
                                receiver,
                            );
                        })
                    {
                        log::warn!("[microphone-wake] detector worker unavailable: {error}");
                        self.worker_exited(token);
                    }
                }
            }
            other => {
                let error = match other {
                    Ok(Err(e)) => e,
                    Err(e) => e.to_string(),
                    _ => unreachable!(),
                };
                log::warn!("[microphone-wake] prepare failed: {error}");
                let mut data = self.data.lock();
                if data.generation == token {
                    data.status.state = "error".into();
                    data.status.error = Some("microphone_wake_unavailable".into());
                    data.retry_at = Instant::now() + Duration::from_secs(30);
                }
            }
        }
    }

    fn worker_exited(&self, token: u64) {
        let capture = {
            let mut data = self.data.lock();
            if data.generation != token
                || data
                    .capture
                    .as_ref()
                    .is_some_and(|c| c.pending_since.is_some())
            {
                return;
            }
            data.generation += 1;
            data.status.state = "error".into();
            data.status.error = Some("microphone_wake_unavailable".into());
            data.retry_at = Instant::now() + Duration::from_secs(5);
            data.capture.take()
        };
        if let Some(capture) = capture {
            capture.stop();
        }
    }
}

fn owner_snapshot(ingress: &Ingress, cancel: &AtomicBool, start: u64, end: u64) -> Option<Vec<u8>> {
    super::microphone_wake_identity::snapshot(&ingress.buffer,cancel,start,end)
}

fn verify_confirmed_owner(
    ingress: &Ingress,
    cancel: &AtomicBool,
    start: u64,
    mut pcm: Vec<u8>,
    local_relation: Option<crate::wake_phrase::LocalPhraseRelation>,
    mut verify: impl FnMut(&[u8]) -> Result<crate::speaker_verification::VerificationResult, String>,
) -> bool {
    loop {
        if cancel.load(Ordering::SeqCst) {
            return false;
        }
        let result = verify(&pcm);
        if cancel.load(Ordering::SeqCst) {
            return false;
        }
        if super::dictation::microphone_wake_owner_allowed(
            true,
            local_relation,
            pcm.len() / 32,
            &result,
        ) {
            return true;
        }
        let Some(next_ms) = super::dictation::microphone_wake_owner_retry_ms(pcm.len() / 32) else {
            return false;
        };
        let Some(next) = owner_snapshot(ingress, cancel, start, start + next_ms as u64 * 32) else {
            return false;
        };
        pcm = next;
    }
}

fn verify_keyword_owner(
    ingress: &Ingress,
    cancel: &AtomicBool,
    found: &crate::wake_phrase::Match,
    candidate_start: u64,
    candidate: Vec<u8>,
    local_relation: Option<crate::wake_phrase::LocalPhraseRelation>,
    mut verify: impl FnMut(&[u8]) -> Result<crate::speaker_verification::VerificationResult, String>,
) -> bool {
    use super::microphone_wake_identity::{keyword_owner_input,KeywordOwnerInput};
    let (start,end,pcm)=match keyword_owner_input(&ingress.buffer,cancel,found) {
        KeywordOwnerInput::Unavailable=>return false,
        KeywordOwnerInput::LegacyShort=> {
            log::info!("[microphone-wake] owner input source=legacy_short_keyword pcm_ms={}",candidate.len()/32);
            return verify_confirmed_owner(ingress,cancel,candidate_start,candidate,local_relation,verify);
        }
        KeywordOwnerInput::Focused {start,end,pcm}=>(start,end,pcm),
    };
    if cancel.load(Ordering::SeqCst) { return false; }
    let result=verify(&pcm);
    if cancel.load(Ordering::SeqCst) { return false; }
    log::info!("[microphone-wake] owner input source=keyword_tokens start_ms={} end_ms={} pcm_ms={} legacy_pcm_ms={}",start/32,end/32,pcm.len()/32,candidate.len()/32);
    super::dictation::microphone_wake_owner_allowed(true,local_relation,pcm.len()/32,&result)
}

fn run_detector(
    inner: Weak<Inner>,
    runtime: Arc<Runtime>,
    token: u64,
    phrase: String,
    ingress: Arc<Ingress>,
    cancel: Arc<AtomicBool>,
    detector: crate::wake_phrase::StreamingDetector,
    receiver: Receiver<Chunk>,
) {
    let mut decoder = Decoder::new(detector, phrase.clone());
    while !cancel.load(Ordering::SeqCst) {
        let chunk = match receiver.recv_timeout(Duration::from_millis(250)) {
            Ok(chunk) => chunk,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if inner.strong_count() == 0 {
                    break;
                }
                continue;
            }
            Err(_) => break,
        };
        if cancel.load(Ordering::SeqCst) {
            break;
        }
        let found = match decoder.accept(chunk, &ingress.buffer) {
            Ok(Some(found)) => found,
            Ok(None) => continue,
            Err(_) => break,
        };
        if cancel.load(Ordering::SeqCst) {
            break;
        }
        if found.matched_keyword.as_deref() != Some(phrase.as_str()) {
            break;
        }
        let Some(seconds) = found
            .audio_end_seconds
            .filter(|s| s.is_finite() && *s >= 0.0)
        else {
            break;
        };
        let keyword_end = (seconds as f64 * 16_000.0).round() as u64 * 2;
        if keyword_end > ingress.buffer.lock().end {
            break;
        }
        let candidate = {
            let mut buffer = ingress.buffer.lock();
            let start = ((found.start_seconds.unwrap_or((seconds - 3.0).max(0.0)) as f64 * 16_000.0)
                .max(0.0) as u64)
                * 2;
            match buffer.range(start.max(buffer.start), keyword_end) {
                Ok(candidate) => {
                    buffer.detected = true;
                    candidate
                }
                Err(_) => break,
            }
        };
        let candidate_start = keyword_end - candidate.len() as u64;
        let local_relation = super::microphone_wake_confirmation::confirm_live_candidate(
            &ingress.buffer,
            &cancel,
            candidate_start,
            keyword_end,
            |pcm, timeout| super::microphone_wake_decoder::confirm_normalized_candidate_with_timeout(pcm, &phrase, timeout),
        );
        if local_relation.is_none() {
            log::info!("[microphone-wake] keyword not independently confirmed generation={token}");
            runtime.reject_candidate(token);
            break;
        }
        if cancel.load(Ordering::SeqCst) {
            break;
        }
        let owner_allowed = verify_keyword_owner(
            &ingress,
            &cancel,
            &found,
            candidate_start,
            candidate,
            local_relation,
            |pcm| crate::speaker_verification::verify(pcm, &phrase),
        );
        if cancel.load(Ordering::SeqCst) {
            break;
        }
        let Some(inner) = inner.upgrade() else {
            break;
        };
        if !owner_allowed {
            log::info!(
                "[microphone-wake] keyword rejected by existing voiceprint gate generation={token}"
            );
            runtime.reject_candidate(token);
            break;
        }
        let valid = {
            let mut data = runtime.data.lock();
            if data.generation != token {
                false
            } else if let Some(capture) = data.capture.as_mut() {
                capture.keyword_end = Some(keyword_end);
                capture.pending_since = Some(Instant::now());
                data.status.state = "starting".into();
                data.status.wakes += 1;
                true
            } else {
                false
            }
        };
        if valid {
            log::info!("[microphone-wake] keyword accepted generation={token} keyword_end_samples={} retained_body_samples={}", keyword_end / 2, ingress.buffer.lock().end.saturating_sub(keyword_end) / 2);
            async_runtime::spawn(async move {
                let _ = super::dictation::begin_session_from_microphone_wake(&inner, token).await;
            });
        }
        break;
    }
}

#[cfg(test)]
pub(crate) fn diagnostic_source_owner_admission(
    pcm: &[u8], found: &crate::wake_phrase::Match,
    verify: impl FnMut(&[u8]) -> Result<crate::speaker_verification::VerificationResult,String>,
) -> bool {
    let (sender,_receiver)=mpsc::sync_channel(1);
    let ingress=Ingress { buffer:Mutex::new(WakeBuffer::default()),pending:Mutex::new(Vec::new()),sender };
    ingress.buffer.lock().append(pcm);
    let start=(found.start_seconds.unwrap() as f64*16000.0).round() as u64*2;
    let end=(found.audio_end_seconds.unwrap() as f64*16000.0).round() as u64*2;
    let candidate=ingress.buffer.lock().range(start,end).unwrap();
    verify_keyword_owner(&ingress,&AtomicBool::new(false),found,start,candidate,
        Some(crate::wake_phrase::LocalPhraseRelation::ExactStart),verify)
}

#[cfg(test)]
pub(crate) fn diagnostic_owner_admission(pcm_ms: usize, score: f32, matched: bool) -> bool {
    super::dictation::microphone_wake_owner_allowed(true,
        Some(crate::wake_phrase::LocalPhraseRelation::ExactStart), pcm_ms,
        &Ok(crate::speaker_verification::VerificationResult {
            matched, owner_matched: matched, score,
            policy: crate::speaker_verification::VerificationPolicy::Enrolled,
        }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prepared() -> Runtime {
        let runtime = Runtime::default();
        let (sender, _) = mpsc::sync_channel(1);
        let (_, errors) = mpsc::channel();
        let mut data = runtime.data.lock();
        data.config = Some(Config {
            enabled: true,
            device: String::new(),
            phrase: "test".into(),
        });
        data.generation = 5;
        data.capture = Some(IdleCapture {
            recorder: Recorder::without_device_for_test(),
            errors,
            ingress: Arc::new(Ingress {
                buffer: Mutex::new(WakeBuffer::default()),
                pending: Mutex::new(Vec::new()),
                sender,
            }),
            cancel: Arc::new(AtomicBool::new(false)),
            keyword_end: Some(0),
            claimed_session: None,
            pending_since: Some(Instant::now()),
        });
        drop(data);
        runtime
    }

    #[test]
    fn microphone_wake_stale_generation_cannot_create_a_session() {
        let runtime = prepared();
        assert!(runtime
            .claim_start(4, || panic!("stale worker created a session"))
            .is_none());
        runtime.stop_idle();
        assert!(runtime
            .claim_start(5, || panic!("cancelled worker created a session"))
            .is_none());
    }

    #[test]
    fn microphone_wake_stale_rejection_cannot_stop_a_replacement_capture() {
        let runtime = prepared();
        runtime.reject_candidate(4);
        assert!(runtime.data.lock().capture.is_some());
        assert_eq!(runtime.data.lock().generation, 5);
        runtime.reject_candidate(5);
        assert!(runtime.data.lock().capture.is_none());
    }

    #[test]
    fn microphone_wake_claim_is_once_only_and_cancelled_handoff_fails_closed() {
        let runtime = prepared();
        let session = new_session_id();
        assert_eq!(runtime.claim_start(5, || Some(session)), Some(session));
        assert!(runtime
            .claim_start(5, || panic!("duplicate start"))
            .is_none());
        runtime.stop_idle();
        struct Sink;
        impl AudioConsumer for Sink {
            fn consume_pcm_chunk(&self, _: &[u8]) {}
        }
        let stale = runtime.adopt(session, Arc::new(Sink), Arc::new(|_| {}), None);
        assert!(
            stale.is_some_and(|result| result.is_err()),
            "cancelled wake must not fall through to a newly opened microphone"
        );
        assert!(runtime
            .adopt(new_session_id(), Arc::new(Sink), Arc::new(|_| {}), None)
            .is_none());
    }

    #[test]
    fn microphone_wake_preview_releases_capture_and_invalidates_pending_keyword() {
        let runtime = prepared();
        runtime.set_preview_active(true);
        assert!(runtime.data.lock().capture.is_none());
        assert!(runtime
            .claim_start(5, || panic!("preview must own microphone"))
            .is_none());
    }

    #[test]
    fn microphone_wake_queue_is_bounded_and_missing_chunks_remain_recoverable() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let ingress = Ingress {
            buffer: Mutex::new(WakeBuffer::default()),
            pending: Mutex::new(Vec::new()),
            sender,
        };
        ingress.consume_pcm_chunk(&vec![1; FEED_BYTES]);
        ingress.consume_pcm_chunk(&vec![2; FEED_BYTES]); // Queue full, ring keeps source.
        assert_eq!(receiver.try_recv().unwrap().start, 0);
        ingress.consume_pcm_chunk(&vec![3; FEED_BYTES]);
        let next = receiver.try_recv().unwrap();
        assert_eq!(next.start, (2 * FEED_BYTES) as u64);
        let recovered = ingress
            .buffer
            .lock()
            .range(FEED_BYTES as u64, next.end)
            .unwrap();
        assert_eq!(
            recovered,
            [vec![2; FEED_BYTES], vec![3; FEED_BYTES]].concat()
        );
    }

    #[test]
    fn microphone_wake_preferences_are_opt_in_and_round_trip() {
        let old: crate::types::UserPreferences =
            serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(!old.microphone_wake_enabled);
        let mut saved = serde_json::to_value(&old).unwrap();
        saved["microphoneWakeEnabled"] = serde_json::json!(true);
        let enabled: crate::types::UserPreferences = serde_json::from_value(saved.clone()).unwrap();
        assert!(enabled.microphone_wake_enabled);
        assert_eq!(serde_json::to_value(enabled).unwrap(), saved);
    }

    #[test]
    fn microphone_wake_short_owner_input_retries_without_consuming_body() {
        use crate::speaker_verification::{VerificationPolicy, VerificationResult};
        let (sender, _) = mpsc::sync_channel(1);
        let ingress = Ingress {
            buffer: Mutex::new(WakeBuffer::default()),
            pending: Mutex::new(Vec::new()),
            sender,
        };
        let prefix = vec![0; 400 * 32];
        let keyword = vec![1; 880 * 32];
        let body = vec![2; 2_000 * 32];
        ingress
            .buffer
            .lock()
            .append(&[prefix.clone(), keyword.clone(), body.clone()].concat());
        let mut windows = Vec::new();
        assert!(verify_confirmed_owner(
            &ingress,
            &AtomicBool::new(false),
            prefix.len() as u64,
            keyword.clone(),
            Some(crate::wake_phrase::LocalPhraseRelation::ExactStart),
            |pcm| {
                let ms = pcm.len() / 32;
                windows.push(ms);
                if ms < 1_000 {
                    Err("short verified input".into())
                } else {
                    Ok(VerificationResult {
                        matched: false,
                        owner_matched: false,
                        score: 0.313291,
                        policy: VerificationPolicy::Enrolled,
                    })
                }
            }
        ));
        assert_eq!(windows.first(), Some(&880));
        assert_eq!(windows.last(), Some(&1800));
        assert!(windows.len() <= 3 && windows.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(
            ingress
                .buffer
                .lock()
                .body_after((prefix.len() + keyword.len()) as u64)
                .unwrap(),
            body
        );
    }

    #[test]
    fn microphone_keyword_owner_does_not_inherit_earlier_or_later_body_owner() {
        use crate::speaker_verification::{VerificationPolicy,VerificationResult};
        for earlier_owner in [false,true] {
            let prefix=vec![if earlier_owner {77}else{0};800*32];
            let word=vec![11;1200*32];
            let body=[vec![0;300*32],vec![77;2000*32]].concat();
            let pcm=[prefix,word,body.clone()].concat();
            let (sender,_receiver)=mpsc::sync_channel(1);
            let ingress=Ingress {buffer:Mutex::new(WakeBuffer::default()),pending:Mutex::new(Vec::new()),sender};
            ingress.buffer.lock().append(&pcm);
            let found=crate::wake_phrase::Match {start_seconds:Some(0.0),keyword_token_start_seconds:Some(1.0),end_seconds:2.0,audio_end_seconds:Some(2.0),matched_keyword:Some("开始录音".into()),origin:crate::wake_phrase::KeywordEvidenceOrigin::PrimaryDetector};
            let verify=|input:&[u8]| {
                let owner=input.contains(&77);
                Ok(VerificationResult {matched:owner,owner_matched:owner,score:if owner {0.9}else{0.1},policy:VerificationPolicy::Enrolled})
            };
            let relation=Some(crate::wake_phrase::LocalPhraseRelation::ExactStart);
            assert!(verify_confirmed_owner(&ingress,&AtomicBool::new(false),0,pcm[..2000*32].to_vec(),relation,verify),"old broad input can inherit earlier speech or a later owner retry");
            assert!(!verify_keyword_owner(&ingress,&AtomicBool::new(false),&found,0,pcm[..2000*32].to_vec(),relation,verify));
            assert_eq!(ingress.buffer.lock().body_after(2000*32).unwrap(),body);
            assert!(verify_keyword_owner(&ingress,&AtomicBool::new(false),&found,0,pcm[..2000*32].to_vec(),relation,|input| {
                assert!(input.contains(&11) && !input.contains(&77));
                Ok(VerificationResult {matched:true,owner_matched:true,score:0.9,policy:VerificationPolicy::Enrolled})
            }));
        }
    }

    #[test]
    fn microphone_keyword_owner_cancellation_cannot_release_after_verification() {
        let (sender,_receiver)=mpsc::sync_channel(1);
        let ingress=Ingress {buffer:Mutex::new(WakeBuffer::default()),pending:Mutex::new(Vec::new()),sender};
        ingress.buffer.lock().append(&vec![0;3000*32]);
        let found=crate::wake_phrase::Match {start_seconds:Some(0.0),keyword_token_start_seconds:Some(1.0),end_seconds:2.0,audio_end_seconds:Some(2.0),matched_keyword:Some("开始录音".into()),origin:crate::wake_phrase::KeywordEvidenceOrigin::PrimaryDetector};
        let cancel=AtomicBool::new(true);
        assert!(!verify_keyword_owner(&ingress,&cancel,&found,0,vec![0;2000*32],Some(crate::wake_phrase::LocalPhraseRelation::ExactStart),|_|panic!("cancelled input must not infer")));
        cancel.store(false,Ordering::SeqCst);
        assert!(!verify_keyword_owner(&ingress,&cancel,&found,0,vec![0;2000*32],Some(crate::wake_phrase::LocalPhraseRelation::ExactStart),|_| {
            cancel.store(true,Ordering::SeqCst);
            Ok(crate::speaker_verification::VerificationResult {matched:true,owner_matched:true,score:0.9,policy:crate::speaker_verification::VerificationPolicy::Enrolled})
        }));
    }

    #[test]
    fn microphone_wake_owner_retry_cancellation_prevents_verification() {
        let (sender, _) = mpsc::sync_channel(1);
        let ingress = Arc::new(Ingress {
            buffer: Mutex::new(WakeBuffer::default()),
            pending: Mutex::new(Vec::new()),
            sender,
        });
        let cancel = Arc::new(AtomicBool::new(true));
        assert!(!verify_confirmed_owner(
            &ingress,
            &cancel,
            0,
            vec![0; 880 * 32],
            Some(crate::wake_phrase::LocalPhraseRelation::ExactStart),
            |_| panic!("cancelled input must not run verifier")
        ));
        cancel.store(false, Ordering::SeqCst);
        let stop = cancel.clone();
        let worker = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            stop.store(true, Ordering::SeqCst);
        });
        assert!(owner_snapshot(&ingress, &cancel, 0, 1800 * 32).is_none());
        worker.join().unwrap();
    }
}
