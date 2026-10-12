//! Streaming ASR providers.
//!
//! Mirrors the Swift `ListenerTypeASR` library. The Volcengine SAUC bigmodel
//! client is the reference implementation; the wire protocol lives in
//! `frame.rs` (binary frame codec) and the session lifecycle in
//! `volcengine.rs`.

pub mod bailian;
pub mod cloud_voiceprint;
pub mod cloud_gateway;
pub(crate) mod content_consensus;
mod frame;
pub mod local;
// Candidate batch engine. Live delivery must first qualify language coverage,
// process isolation and reconciliation with the shared insertion ledger.
#[cfg(target_os = "windows")]
#[allow(dead_code)]
pub(crate) mod multitalker_native;
#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
pub(crate) mod owner_content_extraction;
#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
pub(crate) mod content_recovery_worker;
#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
pub(crate) mod content_recovery_assets;
#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
pub(crate) mod target_speaker_extraction;
#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
mod target_speaker_output;
pub mod volcengine;
mod volcengine_platform;
pub(crate) mod volcengine_transcript;
pub(crate) mod volcengine_speaker_ledger;
#[cfg(target_os = "windows")]
pub(crate) mod volcengine_native_identity;
#[cfg(target_os = "windows")]
pub(crate) mod volcengine_pending_source;
mod volcengine_untimed_merge;
pub mod wav;
pub mod whisper;

pub use bailian::{BailianCredentials, BailianRealtimeASR};
pub use volcengine::{VolcengineCredentials, VolcengineStreamingASR};
#[allow(unused_imports)]
pub use volcengine_platform::{VolcengineStreamingProvider, VolcengineStreamingSession};
pub use whisper::WhisperBatchASR;

/// Sink for raw 16 kHz / 16-bit / mono PCM bytes coming off the recorder.
///
/// The Recorder pushes chunks here as soon as it has them; the ASR session
/// is free to batch internally before flushing to the network.
pub trait AudioConsumer: Send + Sync {
    fn consume_pcm_chunk(&self, pcm: &[u8]);

    /// Evidence-only variant that carries the immutable capture source across
    /// deferred bridges. Providers that do not use pipeline observation keep
    /// the original audio path through the default implementation.
    fn consume_pcm_chunk_with_source(
        &self,
        pcm: &[u8],
        observation: Option<std::sync::Arc<crate::observability::EmbeddedAudioPipelineObservation>>,
        segment_id: Option<u32>,
    ) {
        self.consume_pcm_chunk_with_source_interval(pcm, observation, segment_id, None);
    }

    /// Explicit source-coordinate variant.  The interval travels with this
    /// PCM call; it is never recovered from a shared observation-side queue.
    fn consume_pcm_chunk_with_source_interval(
        &self,
        pcm: &[u8],
        observation: Option<std::sync::Arc<crate::observability::EmbeddedAudioPipelineObservation>>,
        segment_id: Option<u32>,
        source_interval: Option<crate::observability::PcmSourceInterval>,
    ) {
        let _ = (observation, segment_id, source_interval);
        self.consume_pcm_chunk(pcm);
    }
}

/// What the ASR session yielded once the stream closed.
#[derive(Debug, Clone)]
pub struct RawTranscript {
    pub text: String,
    pub duration_ms: u64,
}

/// User-defined hotword the ASR provider may use to bias decoding.
#[derive(Debug, Clone)]
pub struct DictionaryHotword {
    pub phrase: String,
    pub enabled: bool,
}
