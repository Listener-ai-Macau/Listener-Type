//! Adapter from the Volcengine SAUC session engine to the platform host-audio
//! streaming ASR contracts.

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

use denzic_host_audio_v1_core::asr::{
    AsrError, AsrErrorKind, AsrEventSink, AsrTranscript, StreamingAsrProvider, StreamingAsrSession,
};
use tokio::runtime::Runtime;

use super::volcengine::{
    VolcengineASRError, VolcengineCredentials, VolcengineSessionOptions, VolcengineStreamingASR,
    VolcengineStreamingEvent,
};
use super::{AudioConsumer, DictionaryHotword};

pub const PROVIDER_ID: &str = "volcengine";

#[derive(Clone, Debug)]
pub struct VolcengineStreamingProvider {
    credentials: VolcengineCredentials,
    hotwords: Vec<DictionaryHotword>,
    session_options: VolcengineSessionOptions,
}

impl VolcengineStreamingProvider {
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
        }
    }
}

impl StreamingAsrProvider for VolcengineStreamingProvider {
    fn provider_id(&self) -> &str {
        PROVIDER_ID
    }

    fn start_session(
        &self,
        sink: Arc<dyn AsrEventSink>,
    ) -> Result<Box<dyn StreamingAsrSession>, AsrError> {
        if self.credentials.app_id.trim().is_empty()
            || self.credentials.access_token.trim().is_empty()
            || self.credentials.resource_id.trim().is_empty()
        {
            return Err(map_volcengine_error(VolcengineASRError::CredentialsMissing));
        }

        let runtime = Arc::new(Runtime::new().map_err(|error| {
            AsrError::new(
                AsrErrorKind::Unavailable,
                format!("create Volcengine ASR runtime: {error}"),
            )
        })?);
        let asr = Arc::new(VolcengineStreamingASR::new_with_session_options(
            self.credentials.clone(),
            self.hotwords.clone(),
            self.session_options,
        ));
        let seen_final = Arc::new(AtomicBool::new(false));
        let seen_error = Arc::new(AtomicBool::new(false));
        let sink_for_events = Arc::clone(&sink);
        let seen_final_for_events = Arc::clone(&seen_final);
        let seen_error_for_events = Arc::clone(&seen_error);
        asr.set_streaming_event_callback(Some(Arc::new(move |event| match event {
            VolcengineStreamingEvent::Partial(text) => sink_for_events.on_partial(&text),
            VolcengineStreamingEvent::Final(text) => {
                seen_final_for_events.store(true, Ordering::SeqCst);
                sink_for_events.on_final(&text);
            }
            VolcengineStreamingEvent::Error(error) => {
                seen_error_for_events.store(true, Ordering::SeqCst);
                let mapped = map_volcengine_error(error);
                sink_for_events.on_error(&mapped);
            }
        })));

        let asr_for_open = Arc::clone(&asr);
        let open_result = run_on_runtime(Arc::clone(&runtime), async move {
            asr_for_open.open_session().await
        })?;
        open_result.map_err(map_volcengine_error)?;

        Ok(Box::new(VolcengineStreamingSession {
            asr,
            runtime,
            sink,
            seen_final,
            seen_error,
        }))
    }
}

pub struct VolcengineStreamingSession {
    asr: Arc<VolcengineStreamingASR>,
    runtime: Arc<Runtime>,
    sink: Arc<dyn AsrEventSink>,
    seen_final: Arc<AtomicBool>,
    seen_error: Arc<AtomicBool>,
}

impl StreamingAsrSession for VolcengineStreamingSession {
    fn push_pcm(&self, pcm: &[u8]) {
        self.asr.consume_pcm_chunk(pcm);
    }

    fn finish(&self) -> Result<AsrTranscript, AsrError> {
        let asr = Arc::clone(&self.asr);
        let result = match run_on_runtime(Arc::clone(&self.runtime), async move {
            asr.send_last_frame().await?;
            asr.await_final_result().await
        }) {
            Err(error) => {
                if !self.seen_error.swap(true, Ordering::SeqCst) {
                    self.sink.on_error(&error);
                }
                return Err(error);
            }
            Ok(Ok(result)) => result,
            Ok(Err(error)) => {
                let mapped = map_volcengine_error(error);
                if !self.seen_error.swap(true, Ordering::SeqCst) {
                    self.sink.on_error(&mapped);
                }
                return Err(mapped);
            }
        };

        if !self.seen_final.swap(true, Ordering::SeqCst) {
            self.sink.on_final(&result.text);
        }
        Ok(AsrTranscript {
            text: result.text,
            is_final: true,
        })
    }

    fn cancel(&self) {
        self.asr.cancel();
    }
}

impl Drop for VolcengineStreamingSession {
    fn drop(&mut self) {
        if self.asr.is_connected() {
            self.asr.cancel();
        }
    }
}

fn run_on_runtime<T, F>(runtime: Arc<Runtime>, future: F) -> Result<T, AsrError>
where
    T: Send + 'static,
    F: Future<Output = T> + Send + 'static,
{
    thread::Builder::new()
        .name("listener-volcengine-platform".to_string())
        .spawn(move || runtime.block_on(future))
        .map_err(|error| {
            AsrError::new(
                AsrErrorKind::Unavailable,
                format!("spawn Volcengine ASR bridge: {error}"),
            )
        })?
        .join()
        .map_err(|_| {
            AsrError::new(
                AsrErrorKind::Provider,
                "Volcengine ASR bridge thread panicked",
            )
        })
}

fn map_volcengine_error(error: VolcengineASRError) -> AsrError {
    let message = error.to_string();
    match error {
        VolcengineASRError::CredentialsMissing => AsrError::new(AsrErrorKind::Unavailable, message),
        VolcengineASRError::AuthRejected(_) | VolcengineASRError::AuthenticationFailed => {
            AsrError::new(AsrErrorKind::Auth, message)
        }
        VolcengineASRError::QuotaExceeded(_) => AsrError::new(AsrErrorKind::Provider, message),
        VolcengineASRError::ConnectionFailed(_) | VolcengineASRError::FinalResultTimeout => {
            AsrError::retryable(AsrErrorKind::Network, message)
        }
        VolcengineASRError::NoFinalResult
        | VolcengineASRError::FinalResultCoverageIncomplete { .. }
        | VolcengineASRError::DecodeFailed(_) => {
            AsrError::retryable(AsrErrorKind::Provider, message)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoopSink;

    impl AsrEventSink for NoopSink {
        fn on_partial(&self, _text: &str) {}

        fn on_final(&self, _text: &str) {}

        fn on_error(&self, _error: &AsrError) {}
    }

    #[test]
    fn provider_id_matches_listener_configuration_key() {
        let provider = VolcengineStreamingProvider::new(
            VolcengineCredentials {
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: "resource".into(),
            },
            Vec::new(),
        );
        assert_eq!(provider.provider_id(), "volcengine");
    }

    #[test]
    fn missing_credentials_fail_before_runtime_or_network_setup() {
        let provider = VolcengineStreamingProvider::new(
            VolcengineCredentials {
                app_id: String::new(),
                access_token: "token".into(),
                resource_id: "resource".into(),
            },
            Vec::new(),
        );
        let error = match provider.start_session(Arc::new(NoopSink)) {
            Ok(_) => panic!("missing credentials must fail"),
            Err(error) => error,
        };
        assert_eq!(error.kind, AsrErrorKind::Unavailable);
        assert!(!error.retryable);
    }

    #[test]
    fn provider_errors_keep_retry_policy() {
        let network =
            map_volcengine_error(VolcengineASRError::ConnectionFailed("dns failed".into()));
        assert_eq!(network.kind, AsrErrorKind::Network);
        assert!(network.retryable);

        let auth = map_volcengine_error(VolcengineASRError::AuthRejected(401));
        assert_eq!(auth.kind, AsrErrorKind::Auth);
        assert!(!auth.retryable);

        let quota = map_volcengine_error(VolcengineASRError::QuotaExceeded(45_000_292));
        assert_eq!(quota.kind, AsrErrorKind::Provider);
        assert!(!quota.retryable);
    }
}
