use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Instant;

use parking_lot::Mutex;
use serde::Serialize;

use crate::coordinator_state::SessionId;
use crate::observability_v1::{
    new_host_correlation_id, BleLifecycleState, Capability, CommandResult, ErrorCategory,
    EventEnvelope, EventSource, TimingMetric,
};

const FIRMWARE_CORRELATION_PREFIX: u64 = 0x4c53_544e_0000_0000;
const CORRELATION_OPERATION_MASK: u64 = 0x0fff_ffff;

#[derive(Default)]
struct SourceSequences {
    type_events: u32,
    transport_events: u32,
    provider_events: u32,
}

impl SourceSequences {
    fn next(&mut self, source: EventSource) -> u32 {
        let sequence = match source {
            EventSource::Type => &mut self.type_events,
            EventSource::Transport => &mut self.transport_events,
            EventSource::Provider => &mut self.provider_events,
            EventSource::Firmware => unreachable!("Type never emits firmware events"),
        };
        *sequence = sequence.saturating_add(1);
        *sequence
    }
}

struct AudioObservation {
    correlation_id: u64,
    started_at: Instant,
    stopped_at: Option<Instant>,
    first_packet_observed: bool,
    first_preview_observed: bool,
    sequences: SourceSequences,
}

impl AudioObservation {
    fn new(embedded_session_id: u32, started_at: Instant) -> Self {
        Self {
            correlation_id: correlation_for_firmware_operation(
                Capability::Audio,
                embedded_session_id,
            ),
            started_at,
            stopped_at: None,
            first_packet_observed: false,
            first_preview_observed: false,
            sequences: SourceSequences::default(),
        }
    }

    fn next_event(
        &mut self,
        now: Instant,
        source: EventSource,
        capability: Capability,
        lifecycle: BleLifecycleState,
        result: CommandResult,
        error: ErrorCategory,
        timing_metric: TimingMetric,
        timing_value_ms: u32,
    ) -> EventEnvelope {
        EventEnvelope::new(
            self.correlation_id,
            self.sequences.next(source),
            elapsed_ms(self.started_at, now),
            source,
            capability,
        )
        .with_ble_lifecycle_state(lifecycle)
        .with_result(result)
        .with_error(error)
        .with_timing(timing_metric, timing_value_ms)
    }
}

#[derive(Serialize)]
struct LoggedEvent<'a> {
    event: &'a str,
    #[serde(flatten)]
    envelope: EventEnvelope,
}

static AUDIO_OBSERVATIONS: OnceLock<Mutex<HashMap<SessionId, AudioObservation>>> = OnceLock::new();

fn audio_observations() -> &'static Mutex<HashMap<SessionId, AudioObservation>> {
    AUDIO_OBSERVATIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn correlation_for_firmware_operation(capability: Capability, operation_key: u32) -> u64 {
    FIRMWARE_CORRELATION_PREFIX
        | ((capability as u64) << 28)
        | (u64::from(operation_key) & CORRELATION_OPERATION_MASK)
}

fn elapsed_ms(started_at: Instant, now: Instant) -> u32 {
    now.saturating_duration_since(started_at)
        .as_millis()
        .min(u128::from(u32::MAX)) as u32
}

fn emit(event: &'static str, envelope: EventEnvelope) {
    match serde_json::to_string(&LoggedEvent { event, envelope }) {
        Ok(line) => log::info!("[obs-v1] {line}"),
        Err(error) => log::warn!("[obs-v1] failed to serialize event={event}: {error}"),
    }
}

fn classify_error(message: &str) -> ErrorCategory {
    let normalized = message.to_ascii_lowercase();
    if [
        "audio delivery startup",
        "audio delivery readiness",
        "autoassignedsequence",
        "sequence in request",
    ]
    .iter()
    .any(|needle| normalized.contains(needle))
    {
        ErrorCategory::Host
    } else if [
        "api key",
        "credential",
        "unauthorized",
        "forbidden",
        "invalid model",
        "model not found",
        "401",
        "403",
    ]
    .iter()
    .any(|needle| normalized.contains(needle))
    {
        ErrorCategory::Provider
    } else if ["rate limit", "quota", "429"]
        .iter()
        .any(|needle| normalized.contains(needle))
    {
        ErrorCategory::Resource
    } else if [
        "network",
        "dns",
        "socket",
        "websocket",
        "audio delivery drain",
        "proxy",
        "connection refused",
        "connection reset",
        "network unreachable",
    ]
    .iter()
    .any(|needle| normalized.contains(needle))
    {
        ErrorCategory::Network
    } else if ["ble", "gatt", "packet", "pcm", "audio link", "bluetooth"]
        .iter()
        .any(|needle| normalized.contains(needle))
        || message.contains("蓝牙")
        || message.contains("嵌入式音频")
        || message.contains("数据包")
    {
        ErrorCategory::Transport
    } else if ["asr", "volc", "model", "provider", "api key", "credential"]
        .iter()
        .any(|needle| normalized.contains(needle))
        || message.contains("转写")
        || message.contains("模型")
        || message.contains("识别服务")
    {
        ErrorCategory::Provider
    } else if ["microphone", "capture device"]
        .iter()
        .any(|needle| normalized.contains(needle))
        || message.contains("麦克风")
        || message.contains("录音设备")
    {
        ErrorCategory::Device
    } else if ["schema", "protocol", "manifest"]
        .iter()
        .any(|needle| normalized.contains(needle))
        || message.contains("协议")
        || message.contains("校验")
    {
        ErrorCategory::Protocol
    } else if ["busy", "exhausted", "capacity"]
        .iter()
        .any(|needle| normalized.contains(needle))
        || message.contains("资源")
    {
        ErrorCategory::Resource
    } else {
        ErrorCategory::Host
    }
}

fn source_for_error(error: ErrorCategory) -> EventSource {
    match error {
        ErrorCategory::Transport => EventSource::Transport,
        ErrorCategory::Provider | ErrorCategory::Network => EventSource::Provider,
        _ => EventSource::Type,
    }
}

pub(crate) fn begin_embedded_audio_session(session_id: SessionId, embedded_session_id: u32) {
    let now = Instant::now();
    let mut observation = AudioObservation::new(embedded_session_id, now);
    let audio_started = observation.next_event(
        now,
        EventSource::Type,
        Capability::Audio,
        BleLifecycleState::Recording,
        CommandResult::Started,
        ErrorCategory::None,
        TimingMetric::None,
        0,
    );
    let ble_recording = observation.next_event(
        now,
        EventSource::Type,
        Capability::Ble,
        BleLifecycleState::Recording,
        CommandResult::Started,
        ErrorCategory::None,
        TimingMetric::None,
        0,
    );
    audio_observations().lock().insert(session_id, observation);
    emit("embedded_audio_started", audio_started);
    emit("embedded_ble_recording", ble_recording);
}

pub(crate) fn record_embedded_audio_first_packet(session_id: SessionId) {
    let now = Instant::now();
    let event = {
        let mut observations = audio_observations().lock();
        let Some(observation) = observations.get_mut(&session_id) else {
            return;
        };
        if observation.first_packet_observed {
            return;
        }
        observation.first_packet_observed = true;
        let elapsed = elapsed_ms(observation.started_at, now);
        observation.next_event(
            now,
            EventSource::Transport,
            Capability::Audio,
            BleLifecycleState::Recording,
            CommandResult::Started,
            ErrorCategory::None,
            TimingMetric::AudioFirstPacketMs,
            elapsed,
        )
    };
    emit("embedded_audio_first_packet", event);
}

pub(crate) fn record_embedded_audio_stop(session_id: SessionId) {
    let now = Instant::now();
    let event = {
        let mut observations = audio_observations().lock();
        let Some(observation) = observations.get_mut(&session_id) else {
            return;
        };
        if observation.stopped_at.is_some() {
            return;
        }
        observation.stopped_at = Some(now);
        observation.next_event(
            now,
            EventSource::Type,
            Capability::Audio,
            BleLifecycleState::ConnectedIdle,
            CommandResult::Succeeded,
            ErrorCategory::None,
            TimingMetric::None,
            0,
        )
    };
    emit("embedded_audio_stop_received", event);
}

#[derive(Clone, Copy)]
pub(crate) enum PreviewSource {
    Sidecar,
    FinalSupplement,
}

impl PreviewSource {
    fn event_name(self) -> &'static str {
        match self {
            Self::Sidecar => "embedded_audio_preview_first_sidecar",
            Self::FinalSupplement => "embedded_audio_preview_first_final_supplement",
        }
    }
}

pub(crate) fn record_embedded_audio_first_preview(
    session_id: SessionId,
    preview_source: PreviewSource,
) {
    let now = Instant::now();
    let event = {
        let mut observations = audio_observations().lock();
        let Some(observation) = observations.get_mut(&session_id) else {
            return;
        };
        if observation.first_preview_observed {
            return;
        }
        observation.first_preview_observed = true;
        let elapsed = elapsed_ms(observation.started_at, now);
        observation.next_event(
            now,
            EventSource::Provider,
            Capability::Audio,
            BleLifecycleState::Recording,
            CommandResult::Started,
            ErrorCategory::None,
            TimingMetric::PreviewLatencyMs,
            elapsed,
        )
    };
    emit(preview_source.event_name(), event);
}

pub(crate) fn record_embedded_audio_final(session_id: SessionId) {
    let now = Instant::now();
    let event = {
        let mut observations = audio_observations().lock();
        let Some(mut observation) = observations.remove(&session_id) else {
            return;
        };
        let final_started_at = observation.stopped_at.unwrap_or(observation.started_at);
        let elapsed = elapsed_ms(final_started_at, now);
        observation.next_event(
            now,
            EventSource::Provider,
            Capability::Audio,
            BleLifecycleState::ConnectedIdle,
            CommandResult::Succeeded,
            ErrorCategory::None,
            TimingMetric::FinalTranscriptionMs,
            elapsed,
        )
    };
    emit("embedded_audio_final", event);
}

pub(crate) fn record_embedded_audio_failure(session_id: SessionId, message: &str) {
    let now = Instant::now();
    let error = classify_error(message);
    let event = {
        let mut observations = audio_observations().lock();
        let Some(mut observation) = observations.remove(&session_id) else {
            return;
        };
        observation.next_event(
            now,
            source_for_error(error),
            Capability::Audio,
            BleLifecycleState::ConnectedIdle,
            CommandResult::Failed,
            error,
            TimingMetric::None,
            0,
        )
    };
    emit("embedded_audio_failed", event);
}

pub(crate) fn record_embedded_audio_timeout(session_id: SessionId) {
    let now = Instant::now();
    let event = {
        let mut observations = audio_observations().lock();
        let Some(mut observation) = observations.remove(&session_id) else {
            return;
        };
        observation.next_event(
            now,
            EventSource::Type,
            Capability::Audio,
            BleLifecycleState::ConnectedIdle,
            CommandResult::Timeout,
            ErrorCategory::Resource,
            TimingMetric::None,
            0,
        )
    };
    emit("embedded_audio_timeout", event);
}

pub(crate) struct OtaObservation {
    correlation_id: u64,
    started_at: Instant,
    sequences: SourceSequences,
}

impl OtaObservation {
    pub(crate) fn correlation_id(&self) -> u64 {
        self.correlation_id
    }

    fn next_event(
        &mut self,
        now: Instant,
        source: EventSource,
        result: CommandResult,
        error: ErrorCategory,
        timing_metric: TimingMetric,
        timing_value_ms: u32,
    ) -> EventEnvelope {
        EventEnvelope::new(
            self.correlation_id,
            self.sequences.next(source),
            elapsed_ms(self.started_at, now),
            source,
            Capability::Ota,
        )
        .with_ble_lifecycle_state(BleLifecycleState::ConnectedIdle)
        .with_result(result)
        .with_error(error)
        .with_timing(timing_metric, timing_value_ms)
    }

    pub(crate) fn record_transfer_completed(&mut self, transfer_elapsed_ms: u64) {
        let event = self.next_event(
            Instant::now(),
            EventSource::Transport,
            CommandResult::Succeeded,
            ErrorCategory::None,
            TimingMetric::OtaTransferMs,
            u32::try_from(transfer_elapsed_ms).unwrap_or(u32::MAX),
        );
        emit("ota_gatt_transfer_completed", event);
    }

    pub(crate) fn record_transfer_failed(&mut self, transfer_elapsed_ms: u64, message: &str) {
        let error = classify_error(message);
        let event = self.next_event(
            Instant::now(),
            source_for_error(error),
            CommandResult::Failed,
            error,
            TimingMetric::OtaTransferMs,
            u32::try_from(transfer_elapsed_ms).unwrap_or(u32::MAX),
        );
        emit("ota_gatt_transfer_failed", event);
    }

    pub(crate) fn record_control_handoff_failed(&mut self, message: &str) {
        let error = classify_error(message);
        let event = self.next_event(
            Instant::now(),
            source_for_error(error),
            CommandResult::Failed,
            error,
            TimingMetric::None,
            0,
        );
        emit("ota_control_handoff_failed", event);
    }

    pub(crate) fn record_reconnect_confirmation(&mut self, matched: bool) {
        let event = self.next_event(
            Instant::now(),
            EventSource::Type,
            if matched {
                CommandResult::Succeeded
            } else {
                CommandResult::Failed
            },
            if matched {
                ErrorCategory::None
            } else {
                ErrorCategory::Protocol
            },
            TimingMetric::None,
            0,
        );
        emit("ota_reconnect_confirmation", event);
    }
}

pub(crate) fn begin_ota_transfer() -> OtaObservation {
    let now = Instant::now();
    let mut observation = OtaObservation {
        correlation_id: new_host_correlation_id(),
        started_at: now,
        sequences: SourceSequences::default(),
    };
    let event = observation.next_event(
        now,
        EventSource::Type,
        CommandResult::Started,
        ErrorCategory::None,
        TimingMetric::None,
        0,
    );
    emit("ota_transfer_started", event);
    observation
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn embedded_audio_events_share_firmware_correlation_and_distinguish_preview_final_and_ble() {
        let started_at = Instant::now();
        let mut observation = AudioObservation::new(77, started_at);
        let audio_started = observation.next_event(
            started_at,
            EventSource::Type,
            Capability::Audio,
            BleLifecycleState::Recording,
            CommandResult::Started,
            ErrorCategory::None,
            TimingMetric::None,
            0,
        );
        let ble_recording = observation.next_event(
            started_at,
            EventSource::Type,
            Capability::Ble,
            BleLifecycleState::Recording,
            CommandResult::Started,
            ErrorCategory::None,
            TimingMetric::None,
            0,
        );
        let first_packet = observation.next_event(
            started_at + Duration::from_millis(20),
            EventSource::Transport,
            Capability::Audio,
            BleLifecycleState::Recording,
            CommandResult::Started,
            ErrorCategory::None,
            TimingMetric::AudioFirstPacketMs,
            20,
        );
        observation.stopped_at = Some(started_at + Duration::from_millis(80));
        let preview = observation.next_event(
            started_at + Duration::from_millis(120),
            EventSource::Provider,
            Capability::Audio,
            BleLifecycleState::Recording,
            CommandResult::Started,
            ErrorCategory::None,
            TimingMetric::PreviewLatencyMs,
            120,
        );
        let final_event = observation.next_event(
            started_at + Duration::from_millis(240),
            EventSource::Provider,
            Capability::Audio,
            BleLifecycleState::ConnectedIdle,
            CommandResult::Succeeded,
            ErrorCategory::None,
            TimingMetric::FinalTranscriptionMs,
            160,
        );

        assert_eq!(audio_started.correlation_id, ble_recording.correlation_id);
        assert_eq!(audio_started.correlation_id, first_packet.correlation_id);
        assert_eq!(audio_started.correlation_id, preview.correlation_id);
        assert_eq!(audio_started.correlation_id, final_event.correlation_id);
        assert_eq!(ble_recording.capability, Capability::Ble);
        assert_eq!(
            ble_recording.ble_lifecycle_state,
            BleLifecycleState::Recording
        );
        assert_eq!(first_packet.timing_metric, TimingMetric::AudioFirstPacketMs);
        assert_eq!(preview.timing_metric, TimingMetric::PreviewLatencyMs);
        assert_eq!(
            final_event.timing_metric,
            TimingMetric::FinalTranscriptionMs
        );
        assert_eq!(preview.source, EventSource::Provider);
        assert_eq!(final_event.source, EventSource::Provider);
        assert_eq!(preview.event_sequence, 1);
        assert_eq!(final_event.event_sequence, 2);
    }

    #[test]
    fn provider_and_network_failures_are_distinguishable() {
        assert_eq!(
            classify_error("ASR provider rejected the configured model"),
            ErrorCategory::Provider
        );
        assert_eq!(
            classify_error("websocket connection refused by network"),
            ErrorCategory::Network
        );
        assert_eq!(
            classify_error("websocket audio delivery drain did not complete within 2700 ms"),
            ErrorCategory::Network
        );
        assert_eq!(
            classify_error("Volcengine ASR HTTP 401 unauthorized"),
            ErrorCategory::Provider
        );
        assert_eq!(
            classify_error("audio delivery startup did not reach ready state within 1800 ms"),
            ErrorCategory::Host
        );
        assert_eq!(
            classify_error("autoAssignedSequence (26) mismatch sequence in request (27)"),
            ErrorCategory::Host
        );
        assert_eq!(
            source_for_error(ErrorCategory::Provider),
            EventSource::Provider
        );
        assert_eq!(
            source_for_error(ErrorCategory::Network),
            EventSource::Provider
        );
    }

    #[test]
    fn serialized_event_preserves_the_platform_envelope_field_names() {
        let now = Instant::now();
        let mut observation = AudioObservation::new(19, now);
        let envelope = observation.next_event(
            now,
            EventSource::Type,
            Capability::Audio,
            BleLifecycleState::Recording,
            CommandResult::Started,
            ErrorCategory::None,
            TimingMetric::None,
            0,
        );
        let serialized = serde_json::to_value(LoggedEvent {
            event: "embedded_audio_started",
            envelope,
        })
        .expect("observability envelope serializes");

        for field in [
            "contract_version",
            "correlation_id",
            "event_sequence",
            "monotonic_ms",
            "source",
            "capability",
            "ble_lifecycle_state",
            "command_result",
            "error_category",
            "timing_metric",
            "timing_value_ms",
        ] {
            assert!(
                serialized.get(field).is_some(),
                "missing Platform envelope field {field}"
            );
        }
        assert_eq!(serialized["contract_version"], 1);
        assert_eq!(serialized["event"], "embedded_audio_started");
    }

    #[test]
    fn ota_transfers_use_distinct_nonzero_host_correlations() {
        let first = begin_ota_transfer().correlation_id();
        let second = begin_ota_transfer().correlation_id();

        assert_ne!(first, 0);
        assert_ne!(second, 0);
        assert_ne!(first, second);
    }
}
