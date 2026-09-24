use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::Serialize;

use crate::coordinator_state::SessionId;
use crate::observability_v1::{
    new_host_correlation_id, BleLifecycleState, Capability, CommandResult, ErrorCategory,
    EventEnvelope, EventSource, TimingMetric,
};

const FIRMWARE_CORRELATION_PREFIX: u64 = 0x4c53_544e_0000_0000;
const CORRELATION_OPERATION_MASK: u64 = 0x0fff_ffff;

/// A half-open PCM byte interval in one explicitly named source coordinate
/// system.  This is metadata only: it never owns or copies PCM bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct PcmRange {
    pub(crate) start: u64,
    pub(crate) end: u64,
}

impl PcmRange {
    pub(crate) fn from_start_and_bytes(start: u64, bytes: usize) -> Option<Self> {
        let bytes = u64::try_from(bytes).ok()?;
        let end = start.checked_add(bytes)?;
        Some(Self { start, end })
    }

    pub(crate) fn len(self) -> u64 {
        self.end.saturating_sub(self.start)
    }

    pub(crate) fn is_adjacent_to(self, next: Self) -> bool {
        self.end == next.start
    }

    /// Takes a prefix without ever producing an inverted or wrapped range.
    /// The remainder stays in `self`.
    pub(crate) fn take_prefix(&mut self, bytes: usize) -> Option<Self> {
        let bytes = u64::try_from(bytes).ok()?;
        let end = self.start.checked_add(bytes)?;
        if end > self.end {
            return None;
        }
        let prefix = Self {
            start: self.start,
            end,
        };
        self.start = end;
        Some(prefix)
    }
}

/// The coordinate system is explicit: this is the host coordinator's input
/// PCM stream, not a firmware or collector offset.  Two physical sources may
/// both begin at offset zero and must not be merged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PcmStreamKind {
    CoordinatorInputPcm,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct PcmSourceInterval {
    pub(crate) capture_generation: u64,
    pub(crate) source_stream_id: u64,
    pub(crate) stream_kind: PcmStreamKind,
    pub(crate) mapping: PcmMappingKind,
    pub(crate) segment_id: Option<u32>,
    pub(crate) range: PcmRange,
}

/// A fact about one collector-emitted PCM chunk.  The range belongs to the
/// host collector output stream; it is deliberately not a firmware sample
/// interval.  Keeping this fact in the coordinator observation makes packet
/// revision conflicts visible without changing PCM admission yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct CollectorPcmChunkFact {
    pub(crate) session_id: u32,
    pub(crate) packet_sequence: u16,
    pub(crate) pcm_bytes: u64,
    pub(crate) metadata: crate::embedded_audio::StreamingPcmChunkMetadata,
}

impl PcmSourceInterval {
    pub(crate) fn len(self) -> u64 {
        self.range.len()
    }

    pub(crate) fn is_legal_for_source(
        self,
        observation: Option<&EmbeddedAudioPipelineObservation>,
        segment_id: Option<u32>,
        pcm_bytes: usize,
    ) -> bool {
        let Some(observation) = observation else {
            return false;
        };
        let Some(pcm_bytes) = u64::try_from(pcm_bytes).ok() else {
            return false;
        };
        self.source_stream_id != 0
            && self.stream_kind == PcmStreamKind::CoordinatorInputPcm
            && self.mapping == PcmMappingKind::PositionPreserving
            && self.capture_generation == observation.capture_generation()
            && self.segment_id == segment_id
            && self.range.start <= self.range.end
            && self.range.end.checked_sub(self.range.start) == Some(pcm_bytes)
    }

    pub(crate) fn same_stream_and_adjacent(self, next: Self) -> bool {
        self.capture_generation == next.capture_generation
            && self.source_stream_id == next.source_stream_id
            && self.stream_kind == next.stream_kind
            && self.mapping == next.mapping
            && self.segment_id == next.segment_id
            && self.range.is_adjacent_to(next.range)
    }

    pub(crate) fn take_prefix(&mut self, bytes: usize) -> Option<Self> {
        let range = self.range.take_prefix(bytes)?;
        Some(Self { range, ..*self })
    }
}

/// The coordinator pipeline has separate byte coordinates for accepted input,
/// the optional debug archive, and normalized ASR output.  They share source
/// evidence but are not interchangeable offsets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PcmStage {
    CoordinatorAccepted,
    Archive,
    Normalized,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PcmStageDisposition {
    Appended,
    Disabled,
    Empty,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PcmFormat {
    PcmS16LeMono16k,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct PcmStageMappingFact {
    pub(crate) logical_stream_id: u64,
    /// One normalized/accepted operation may have several local source
    /// projections. This identity distinguishes those projections from a
    /// second output operation.
    pub(crate) operation_id: Option<u64>,
    pub(crate) stage: PcmStage,
    pub(crate) stream_kind: PcmStreamKind,
    pub(crate) pcm_format: PcmFormat,
    pub(crate) mapping: PcmMappingKind,
    pub(crate) disposition: PcmStageDisposition,
    pub(crate) destination_range: Option<PcmRange>,
    pub(crate) source_shares: Vec<AsrSourceShareFact>,
}

/// A length-preserving PCM transform changes samples but not positions.  It
/// must not be described as content identity; future length-changing paths
/// must use `Unknown` until they provide an explicit mapping.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PcmMappingKind {
    PositionPreserving,
    Unknown,
}

const ASR_DESTINATION_FACT_CAPACITY: usize = 256;
const ASR_DESTINATION_SHARES_PER_FACT_CAPACITY: usize = 128;
const PCM_STAGE_FACT_CAPACITY: usize = 768;
const PCM_STAGE_SHARES_PER_FACT_CAPACITY: usize = 128;
const COLLECTOR_PCM_FACT_CAPACITY: usize = 768;

/// A bounded, capture-independent projection owned by one coordinator
/// logical stream. A capture observation may mirror these facts, but missing
/// capture plumbing must never erase a known destination range.
#[derive(Default)]
pub(crate) struct PcmStageMappingLedger {
    facts: VecDeque<PcmStageMappingFact>,
    fact_update_drop_count: u64,
    fact_update_drop_bytes: u64,
    share_drop_count: u64,
    share_drop_bytes: u64,
    incomplete: bool,
}

impl PcmStageMappingLedger {
    pub(crate) fn record(&mut self, mut fact: PcmStageMappingFact) {
        let fact_bytes = fact
            .source_shares
            .iter()
            .fold(0_u64, |total, share| total.saturating_add(share.bytes));
        let dropped_share_count =
            fact.source_shares
                .len()
                .saturating_sub(PCM_STAGE_SHARES_PER_FACT_CAPACITY) as u64;
        let dropped_share_bytes = fact
            .source_shares
            .iter()
            .skip(PCM_STAGE_SHARES_PER_FACT_CAPACITY)
            .fold(0_u64, |total, share| total.saturating_add(share.bytes));
        let valid_disabled_stage = fact.disposition == PcmStageDisposition::Disabled
            && fact.mapping == PcmMappingKind::Unknown
            && fact.destination_range.is_none();
        if !valid_disabled_stage
            && (matches!(
                fact.disposition,
                PcmStageDisposition::Unknown | PcmStageDisposition::Empty
            ) || fact.mapping == PcmMappingKind::Unknown
                || (fact.disposition == PcmStageDisposition::Appended
                    && fact.destination_range.is_none()))
        {
            self.incomplete = true;
        }
        if dropped_share_count > 0 {
            fact.source_shares
                .truncate(PCM_STAGE_SHARES_PER_FACT_CAPACITY);
            self.share_drop_count = self.share_drop_count.saturating_add(dropped_share_count);
            self.share_drop_bytes = self.share_drop_bytes.saturating_add(dropped_share_bytes);
            self.incomplete = true;
        }
        if self.facts.len() >= PCM_STAGE_FACT_CAPACITY {
            self.fact_update_drop_count = self.fact_update_drop_count.saturating_add(1);
            self.fact_update_drop_bytes = self.fact_update_drop_bytes.saturating_add(fact_bytes);
            self.incomplete = true;
            return;
        }
        self.facts.push_back(fact);
    }

    pub(crate) fn mark_incomplete(&mut self) {
        self.incomplete = true;
    }

    pub(crate) fn facts(&self) -> Vec<PcmStageMappingFact> {
        self.facts.iter().cloned().collect()
    }

    pub(crate) fn capacity_drops(&self) -> (u64, u64, u64, u64, bool) {
        (
            self.fact_update_drop_count,
            self.fact_update_drop_bytes,
            self.share_drop_count,
            self.share_drop_bytes,
            self.incomplete,
        )
    }
}

/// Candidate coordinates are intentionally distinct from coordinator PCM
/// coordinates. A candidate may be inspected by KWS and later released to a
/// new coordinator stream, so reusing `CoordinatorInputPcm` here would create
/// a false source mapping.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct CandidateRange {
    pub(crate) start: u64,
    pub(crate) end: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CandidateFactKind {
    Buffered,
    KwsFed,
    KwsFeedUnknown,
    ReleaseAttempted,
    ReleaseAccepted,
    ReleaseOutcomeUnknown,
    ReleaseRejected,
    Rejected,
    Discarded,
    Closed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct CandidateSourceRunFact {
    pub(crate) capture_generation: Option<u64>,
    pub(crate) segment_id: Option<u32>,
    pub(crate) candidate_range: Option<CandidateRange>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) collector_metadata: Option<crate::embedded_audio::StreamingPcmChunkMetadata>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) collector_emitted_range: Option<crate::embedded_audio::StreamingPcmRange>,
    pub(crate) bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct CandidateFact {
    pub(crate) candidate_id: u64,
    pub(crate) operation_id: Option<u64>,
    pub(crate) kind: CandidateFactKind,
    pub(crate) candidate_range: Option<CandidateRange>,
    pub(crate) kws_feed_range: Option<CandidateRange>,
    pub(crate) release_destination_range: Option<PcmRange>,
    pub(crate) release_coordinator_session_id: Option<String>,
    pub(crate) release_source_stream_id: Option<u64>,
    pub(crate) source_runs: Vec<CandidateSourceRunFact>,
    pub(crate) bytes: u64,
    pub(crate) reason: Option<String>,
}

const CANDIDATE_FACT_CAPACITY: usize = 512;
const CANDIDATE_SOURCE_RUNS_PER_FACT_CAPACITY: usize = 128;

/// Candidate-owned bounded facts. This remains useful when capture tracing is
/// absent; an observation can mirror a fact when one is available.
#[derive(Default)]
pub(crate) struct CandidateFactLedger {
    facts: VecDeque<CandidateFact>,
    fact_drop_count: u64,
    fact_drop_bytes: u64,
    source_run_drop_count: u64,
    source_run_drop_bytes: u64,
    incomplete: bool,
}

impl CandidateFactLedger {
    pub(crate) fn record(&mut self, mut fact: CandidateFact) {
        let fact_bytes = fact.bytes;
        let dropped_count =
            fact.source_runs
                .len()
                .saturating_sub(CANDIDATE_SOURCE_RUNS_PER_FACT_CAPACITY) as u64;
        let dropped_bytes = fact
            .source_runs
            .iter()
            .skip(CANDIDATE_SOURCE_RUNS_PER_FACT_CAPACITY)
            .fold(0_u64, |total, run| total.saturating_add(run.bytes));
        if fact.candidate_range.is_none()
            || (matches!(
                fact.kind,
                CandidateFactKind::KwsFed | CandidateFactKind::KwsFeedUnknown
            ) && fact.kws_feed_range.is_none())
            || fact.kind == CandidateFactKind::KwsFeedUnknown
            || fact
                .source_runs
                .iter()
                .any(|source_run| source_run.candidate_range.is_none())
        {
            self.incomplete = true;
        }
        if fact.kind == CandidateFactKind::ReleaseAccepted
            && fact.release_destination_range.is_none()
        {
            self.incomplete = true;
        }
        if fact.kind == CandidateFactKind::ReleaseOutcomeUnknown {
            self.incomplete = true;
        }
        if dropped_count > 0 {
            fact.source_runs
                .truncate(CANDIDATE_SOURCE_RUNS_PER_FACT_CAPACITY);
            self.source_run_drop_count = self.source_run_drop_count.saturating_add(dropped_count);
            self.source_run_drop_bytes = self.source_run_drop_bytes.saturating_add(dropped_bytes);
            self.incomplete = true;
        }
        if self.facts.len() >= CANDIDATE_FACT_CAPACITY {
            self.fact_drop_count = self.fact_drop_count.saturating_add(1);
            self.fact_drop_bytes = self.fact_drop_bytes.saturating_add(fact_bytes);
            self.incomplete = true;
            return;
        }
        self.facts.push_back(fact);
    }

    pub(crate) fn mark_incomplete(&mut self) {
        self.incomplete = true;
    }

    pub(crate) fn facts(&self) -> Vec<CandidateFact> {
        self.facts.iter().cloned().collect()
    }

    pub(crate) fn is_incomplete(&self) -> bool {
        self.incomplete
    }

    pub(crate) fn capacity_drops(&self) -> (u64, u64, u64, u64, bool) {
        (
            self.fact_drop_count,
            self.fact_drop_bytes,
            self.source_run_drop_count,
            self.source_run_drop_bytes,
            self.incomplete,
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AsrDestinationOutcome {
    Queued,
    Claimed,
    SocketSendCompleted,
    QueueRejected,
    QueueRejectedWithoutQueue,
    SendFailed,
    Abandoned,
    BufferedUnresolved,
    Unresolved,
    /// More than one incompatible terminal outcome was observed for the
    /// same `(asr_stream_id, sequence)` operation.  Keep the conflict
    /// explicit instead of letting a late callback silently rewrite history.
    Conflict,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct AsrSourceShareFact {
    pub(crate) segment_id: Option<u32>,
    pub(crate) bytes: u64,
    pub(crate) destination_range: Option<PcmRange>,
    pub(crate) source_interval: Option<PcmSourceInterval>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) collector_metadata: Option<crate::embedded_audio::StreamingPcmChunkMetadata>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) collector_emitted_range: Option<crate::embedded_audio::StreamingPcmRange>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct AsrDestinationFact {
    pub(crate) asr_stream_id: u64,
    pub(crate) sequence: Option<i32>,
    pub(crate) destination_range: Option<PcmRange>,
    pub(crate) outcome: AsrDestinationOutcome,
    pub(crate) source_shares: Vec<AsrSourceShareFact>,
}

fn asr_destination_outcome_is_terminal(outcome: AsrDestinationOutcome) -> bool {
    matches!(
        outcome,
        AsrDestinationOutcome::SocketSendCompleted
            | AsrDestinationOutcome::QueueRejected
            | AsrDestinationOutcome::QueueRejectedWithoutQueue
            | AsrDestinationOutcome::SendFailed
            | AsrDestinationOutcome::Abandoned
            | AsrDestinationOutcome::BufferedUnresolved
            | AsrDestinationOutcome::Unresolved
            | AsrDestinationOutcome::Conflict
    )
}

/// Merge lifecycle observations without allowing a late callback to regress
/// a settled operation.  A genuinely incompatible terminal outcome is made
/// visible as `Conflict`; the snapshot must never present the last callback as
/// if it were the truth.
fn merge_asr_destination_outcome(
    current: AsrDestinationOutcome,
    incoming: AsrDestinationOutcome,
) -> (AsrDestinationOutcome, bool) {
    if current == incoming || current == AsrDestinationOutcome::Conflict {
        return (current, false);
    }
    if asr_destination_outcome_is_terminal(current) {
        if asr_destination_outcome_is_terminal(incoming) {
            return (AsrDestinationOutcome::Conflict, true);
        }
        // A queued/claimed callback arrived after settlement. Preserve the
        // terminal fact and expose the lifecycle contradiction.
        return (current, true);
    }
    if asr_destination_outcome_is_terminal(incoming) {
        return (incoming, false);
    }
    match (current, incoming) {
        (AsrDestinationOutcome::Queued, AsrDestinationOutcome::Claimed) => {
            (AsrDestinationOutcome::Claimed, false)
        }
        (AsrDestinationOutcome::Claimed, AsrDestinationOutcome::Queued) => {
            // Never downgrade claimed to queued, even if callbacks are
            // delivered out of order.
            (AsrDestinationOutcome::Claimed, true)
        }
        _ => (incoming, true),
    }
}

#[derive(Default)]
struct SourceSequences {
    type_events: u32,
    transport_events: u32,
    provider_events: u32,
}

#[cfg(test)]
mod pipeline_observation_tests {
    use super::*;

    #[test]
    fn start_without_follow_up_keeps_zero_audio_progress() {
        let observation = EmbeddedAudioPipelineObservation::new(100);
        observation.snapshot("capture_start", true);

        let counters = observation.counters_for_test();
        assert_eq!(counters.raw_notify_count, 0);
        assert_eq!(counters.capture_audio_count, 0);
        assert_eq!(counters.coordinator_pcm_bytes, 0);
        assert_eq!(counters.consumer_accepted_pcm_bytes, 0);
        assert_eq!(counters.asr_queued_pcm_bytes, 0);
    }

    #[test]
    fn normalize_reject_does_not_create_downstream_progress() {
        let observation = EmbeddedAudioPipelineObservation::new(101);
        observation.record_raw_notification(12);
        observation.record_channel_forward(true);
        observation.record_normalize(12, None);

        let counters = observation.counters_for_test();
        assert_eq!(counters.raw_notify_count, 1);
        assert_eq!(counters.raw_notify_bytes, 12);
        assert_eq!(counters.normalize_reject_count, 1);
        assert_eq!(counters.normalize_success_count, 0);
        assert_eq!(counters.capture_audio_count, 0);
        assert_eq!(counters.coordinator_channel_received_count, 0);
        assert_eq!(counters.asr_queued_frame_count, 0);
    }

    #[test]
    fn normalize_counters_preserve_distinct_input_and_output_sizes() {
        let observation = EmbeddedAudioPipelineObservation::new(107);
        observation.record_normalize(20, Some(12));

        let counters = observation.counters_for_test();
        assert_eq!(counters.normalize_input_bytes, 20);
        assert_eq!(counters.normalize_output_bytes, 12);
        assert_eq!(counters.normalize_success_count, 1);
    }

    #[test]
    fn capture_foreign_and_duplicate_events_remain_distinguishable() {
        let observation = EmbeddedAudioPipelineObservation::new(102);
        observation.record_capture_event(&crate::embedded_audio::SessionEvent::Ignored(
            crate::embedded_audio::IgnoredPacketReason::ForeignSession,
        ));
        observation.record_capture_event(&crate::embedded_audio::SessionEvent::Ignored(
            crate::embedded_audio::IgnoredPacketReason::DuplicateOrShorterPacket,
        ));

        let counters = observation.counters_for_test();
        assert_eq!(counters.capture_ignored_count, 2);
        assert_eq!(counters.capture_ignored_foreign_count, 1);
        assert_eq!(counters.capture_ignored_duplicate_count, 1);
        assert_eq!(counters.capture_audio_count, 0);
    }

    #[test]
    fn channel_consumption_and_asr_delivery_counters_are_separate() {
        let observation = EmbeddedAudioPipelineObservation::new(103);
        observation.bind_sessions(None, Some(99));
        observation.record_coordinator_channel_forward(true);
        observation.record_coordinator_channel_forward(false);
        observation.record_coordinator_channel_received(6400);
        observation.record_consumer_result(true, 6400);
        observation.record_asr_queued_for_segment(Some(99), 3200);
        observation.record_asr_send_completed_for_segment(Some(99), 3200);
        observation.record_asr_send_failed_for_segment(Some(99), 3200);
        observation.record_asr_queue_rejected(640);

        let counters = observation.counters_for_test();
        assert_eq!(counters.coordinator_channel_forward_success_count, 1);
        assert_eq!(counters.coordinator_channel_forward_failure_count, 1);
        assert_eq!(counters.coordinator_channel_received_bytes, 6400);
        assert_eq!(counters.consumer_accepted_pcm_bytes, 6400);
        assert_eq!(counters.asr_queued_pcm_bytes, 3200);
        assert_eq!(counters.asr_send_completed_pcm_bytes, 3200);
        assert_eq!(counters.asr_send_failed_pcm_bytes, 3200);
        assert_eq!(counters.asr_pending_frame_count, 0);
        assert_eq!(counters.asr_pending_pcm_bytes, 0);
        assert_eq!(counters.asr_queue_rejected_pcm_bytes, 640);
    }

    #[test]
    fn capture_registry_does_not_allow_old_generation_to_pollute_new_one() {
        let old_guard = begin_embedded_audio_pipeline_capture(104);
        let old_observation = old_guard.observation();
        old_observation.record_raw_notification(4);
        drop(old_guard);
        assert!(pipeline_observation(104).is_none());

        let new_guard = begin_embedded_audio_pipeline_capture(105);
        let new_observation = new_guard.observation();
        assert_eq!(new_observation.capture_generation, 105);
        assert_eq!(new_observation.counters_for_test().raw_notify_count, 0);
    }

    #[test]
    fn carried_source_survives_registry_removal_and_rebind() {
        let old_guard = begin_embedded_audio_pipeline_capture(111);
        let old_observation = old_guard.observation();
        old_observation.record_capture_event(&crate::embedded_audio::SessionEvent::Started {
            session_id: 41,
            origin: crate::embedded_audio::SessionStartOrigin::User,
        });
        drop(old_guard);

        let new_guard = begin_embedded_audio_pipeline_capture(112);
        let new_observation = new_guard.observation();
        old_observation.record_asr_queued_for_segment(Some(41), 3_200);
        old_observation.record_asr_send_completed_for_segment(Some(41), 3_200);

        assert_eq!(
            old_observation.asr_delivery_counts_for_test(41),
            (3_200, 3_200, 0, 0)
        );
        assert_eq!(new_observation.counters_for_test().asr_queued_pcm_bytes, 0);
    }

    #[test]
    fn settled_delivery_is_visible_in_a_post_capture_final_snapshot() {
        let guard = begin_embedded_audio_pipeline_capture(113);
        let observation = guard.observation();
        let snapshots = Arc::new(Mutex::new(Vec::<String>::new()));
        let snapshots_for_sink = Arc::clone(&snapshots);
        observation.set_snapshot_sink_for_test(Arc::new(move |payload| {
            snapshots_for_sink.lock().push(payload);
        }));

        observation.record_capture_event(&crate::embedded_audio::SessionEvent::Started {
            session_id: 51,
            origin: crate::embedded_audio::SessionStartOrigin::User,
        });
        observation.record_capture_event(&crate::embedded_audio::SessionEvent::Stopped {
            session_id: 51,
            expected_packet_count: 1,
            origin: crate::embedded_audio::SessionStopOrigin::User,
        });
        observation.record_asr_queued_for_segment(Some(51), 3_200);
        drop(guard);
        observation.record_asr_send_completed_for_segment(Some(51), 3_200);

        let payloads = snapshots.lock();
        assert!(payloads
            .iter()
            .any(|payload| payload.contains("\"reason\":\"capture_end\"")));
        let final_snapshot = payloads
            .iter()
            .find(|payload| payload.contains("\"reason\":\"asr_delivery_settled\""))
            .expect("settled delivery must emit a final snapshot");
        let parsed: serde_json::Value = serde_json::from_str(final_snapshot).unwrap();
        assert_eq!(
            parsed["counters"]["asr_pending_pcm_bytes"].as_u64(),
            Some(0)
        );
    }

    #[test]
    fn mixed_source_delivery_emits_final_snapshot_when_each_source_settles() {
        let guard = begin_embedded_audio_pipeline_capture(114);
        let observation = guard.observation();
        let snapshots = Arc::new(Mutex::new(Vec::<String>::new()));
        let snapshots_for_sink = Arc::clone(&snapshots);
        observation.set_snapshot_sink_for_test(Arc::new(move |payload| {
            snapshots_for_sink.lock().push(payload);
        }));

        for session_id in [61, 62] {
            observation.record_capture_event(&crate::embedded_audio::SessionEvent::Started {
                session_id,
                origin: crate::embedded_audio::SessionStartOrigin::User,
            });
            observation.record_capture_event(&crate::embedded_audio::SessionEvent::Stopped {
                session_id,
                expected_packet_count: 0,
                origin: crate::embedded_audio::SessionStopOrigin::User,
            });
        }
        let shares = [(Some(61), 1_600), (Some(62), 1_600)];
        observation.record_asr_queued_for_sources(&shares, 3_200);
        drop(guard);
        observation.record_asr_send_completed_for_sources(&shares, 3_200);

        assert!(snapshots
            .lock()
            .iter()
            .any(|payload| payload.contains("\"reason\":\"asr_delivery_settled\"")));
    }

    #[test]
    fn late_delivery_after_capture_end_emits_settled_snapshot_without_stop() {
        let guard = begin_embedded_audio_pipeline_capture(116);
        let observation = guard.observation();
        let snapshots = Arc::new(Mutex::new(Vec::<String>::new()));
        let snapshots_for_sink = Arc::clone(&snapshots);
        observation.set_snapshot_sink_for_test(Arc::new(move |payload| {
            snapshots_for_sink.lock().push(payload);
        }));

        observation.record_asr_queued_for_segment(Some(63), 3_200);
        drop(guard);
        observation.record_asr_send_completed_for_segment(Some(63), 3_200);

        let payloads = snapshots.lock();
        let final_snapshot = payloads
            .iter()
            .find(|payload| payload.contains("\"reason\":\"asr_delivery_settled\""))
            .expect("late delivery must emit a settled snapshot without STOP");
        let parsed: serde_json::Value = serde_json::from_str(final_snapshot).unwrap();
        assert_eq!(
            parsed["counters"]["asr_pending_pcm_bytes"].as_u64(),
            Some(0)
        );
        assert!(parsed["segment_counters"]["63"].is_object());
    }

    #[test]
    fn live_chunk_completion_does_not_emit_a_full_final_snapshot() {
        let guard = begin_embedded_audio_pipeline_capture(117);
        let observation = guard.observation();
        let snapshots = Arc::new(Mutex::new(Vec::<String>::new()));
        let snapshots_for_sink = Arc::clone(&snapshots);
        observation.set_snapshot_sink_for_test(Arc::new(move |payload| {
            snapshots_for_sink.lock().push(payload);
        }));

        observation.record_asr_queued_for_segment(Some(64), 3_200);
        observation.record_asr_send_completed_for_segment(Some(64), 3_200);
        assert!(!snapshots
            .lock()
            .iter()
            .any(|payload| payload.contains("\"reason\":\"asr_delivery_settled\"")));

        observation.record_asr_queued_for_segment(Some(64), 3_200);
        drop(guard);
        observation.record_asr_send_completed_for_segment(Some(64), 3_200);
        assert_eq!(
            snapshots
                .lock()
                .iter()
                .filter(|payload| payload.contains("\"reason\":\"asr_delivery_settled\""))
                .count(),
            1
        );
    }

    #[test]
    fn segment_counters_keep_two_physical_segments_separate() {
        let observation = EmbeddedAudioPipelineObservation::new(106);
        observation.record_capture_event(&crate::embedded_audio::SessionEvent::Started {
            session_id: 11,
            origin: crate::embedded_audio::SessionStartOrigin::User,
        });
        observation.record_capture_event(&crate::embedded_audio::SessionEvent::AudioData {
            session_id: 11,
            packet_sequence: 1,
            pcm_bytes: 320,
        });
        observation.record_capture_event(&crate::embedded_audio::SessionEvent::Started {
            session_id: 12,
            origin: crate::embedded_audio::SessionStartOrigin::User,
        });
        observation.record_capture_event(&crate::embedded_audio::SessionEvent::AudioData {
            session_id: 12,
            packet_sequence: 1,
            pcm_bytes: 640,
        });

        let first = observation.segment_counters_for_test(11);
        let second = observation.segment_counters_for_test(12);
        assert_eq!(first.capture_audio_payload_bytes, 320);
        assert_eq!(second.capture_audio_payload_bytes, 640);
        assert_eq!(
            observation.counters_for_test().capture_audio_payload_bytes,
            960
        );
    }

    #[test]
    fn segment_ledger_handles_non_monotonic_physical_ids_without_rebinding() {
        let observation = EmbeddedAudioPipelineObservation::new(115);
        for (session_id, pcm_bytes) in [(900, 320), (7, 640), (101, 960)] {
            observation.record_capture_event(&crate::embedded_audio::SessionEvent::Started {
                session_id,
                origin: crate::embedded_audio::SessionStartOrigin::User,
            });
            observation.record_capture_event(&crate::embedded_audio::SessionEvent::AudioData {
                session_id,
                packet_sequence: 1,
                pcm_bytes,
            });
        }

        assert_eq!(
            observation
                .segment_counters_for_test(900)
                .capture_audio_payload_bytes,
            320
        );
        assert_eq!(
            observation
                .segment_counters_for_test(7)
                .capture_audio_payload_bytes,
            640
        );
        assert_eq!(
            observation
                .segment_counters_for_test(101)
                .capture_audio_payload_bytes,
            960
        );
        assert!(!observation.state.lock().segment_ledger_incomplete);
    }

    #[test]
    fn high_frequency_pcm_events_only_accumulate_until_a_bounded_snapshot() {
        let observation = EmbeddedAudioPipelineObservation::new(108);
        for sequence in 0..100 {
            observation.record_capture_event(&crate::embedded_audio::SessionEvent::AudioData {
                session_id: 13,
                packet_sequence: sequence,
                pcm_bytes: 320,
            });
        }

        let state = observation.state.lock();
        assert_eq!(state.counters.capture_audio_count, 100);
        assert_eq!(state.snapshot_seq, 0);
    }

    #[test]
    fn coordinator_records_collector_metadata_and_marks_missing_as_unknown() {
        let observation = EmbeddedAudioPipelineObservation::new(116);
        let metadata = crate::embedded_audio::StreamingPcmChunkMetadata {
            collector_instance_id: Some(7),
            segment_ordinal: 2,
            packet_sequence: 11,
            emission_ordinal: 4,
            emitted_range: crate::embedded_audio::StreamingPcmRange { start: 8, end: 12 },
            packet_revision: 1,
            packet_disposition: crate::embedded_audio::StreamingPcmChunkDisposition::Replacement,
            wire_payload_bytes: 4,
            declared_pcm_bytes: 4,
            expanded_pcm_bytes: 4,
            previous_emission_ordinal: Some(3),
            previous_emitted_range: Some(crate::embedded_audio::StreamingPcmRange {
                start: 0,
                end: 4,
            }),
            revision_conflict: true,
            metadata_incomplete: false,
        };

        observation.record_coordinator_event(
            &crate::embedded_audio::StreamingSessionEvent::PcmChunk(
                crate::embedded_audio::StreamingPcmChunk {
                    session_id: 701,
                    packet_sequence: 9,
                    pcm: vec![1, 2, 3, 4],
                    raw_input_level_percent: None,
                    after_stop_boundary: false,
                    metadata: Some(metadata),
                },
            ),
        );

        let facts = observation.collector_pcm_facts_for_test();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].session_id, 701);
        assert_eq!(facts[0].packet_sequence, 9);
        assert_eq!(facts[0].pcm_bytes, 4);
        assert_eq!(facts[0].metadata, metadata);
        assert_eq!(
            observation.collector_pcm_fact_capacity_drops_for_test(),
            (0, 0, false)
        );

        observation.record_coordinator_event(
            &crate::embedded_audio::StreamingSessionEvent::PcmChunk(
                crate::embedded_audio::StreamingPcmChunk {
                    session_id: 701,
                    packet_sequence: 10,
                    pcm: vec![5, 6],
                    raw_input_level_percent: None,
                    after_stop_boundary: false,
                    metadata: None,
                },
            ),
        );
        assert_eq!(
            observation.collector_pcm_fact_capacity_drops_for_test(),
            (0, 0, true)
        );
    }

    #[test]
    fn collector_metadata_ledger_overflow_is_explicitly_incomplete() {
        let observation = EmbeddedAudioPipelineObservation::new(121);
        for emission_ordinal in 0..=COLLECTOR_PCM_FACT_CAPACITY as u64 {
            let metadata = crate::embedded_audio::StreamingPcmChunkMetadata {
                collector_instance_id: Some(8),
                segment_ordinal: 1,
                packet_sequence: emission_ordinal as u16,
                emission_ordinal,
                emitted_range: crate::embedded_audio::StreamingPcmRange {
                    start: emission_ordinal * 2,
                    end: emission_ordinal * 2 + 2,
                },
                packet_revision: 0,
                packet_disposition: crate::embedded_audio::StreamingPcmChunkDisposition::New,
                wire_payload_bytes: 2,
                declared_pcm_bytes: 2,
                expanded_pcm_bytes: 2,
                previous_emission_ordinal: None,
                previous_emitted_range: None,
                revision_conflict: false,
                metadata_incomplete: false,
            };
            observation.record_coordinator_event(
                &crate::embedded_audio::StreamingSessionEvent::PcmChunk(
                    crate::embedded_audio::StreamingPcmChunk {
                        session_id: 702,
                        packet_sequence: emission_ordinal as u16,
                        pcm: vec![0, 0],
                        raw_input_level_percent: None,
                        after_stop_boundary: false,
                        metadata: Some(metadata),
                    },
                ),
            );
        }

        assert_eq!(
            observation.collector_pcm_facts_for_test().len(),
            COLLECTOR_PCM_FACT_CAPACITY
        );
        assert_eq!(
            observation.collector_pcm_fact_capacity_drops_for_test(),
            (1, 2, true)
        );
    }

    #[test]
    fn unclosed_segments_are_not_evicted_when_segment_ledger_reaches_capacity() {
        let observation = EmbeddedAudioPipelineObservation::new(109);
        for segment_id in 0..32 {
            observation.record_capture_event(&crate::embedded_audio::SessionEvent::Started {
                session_id: segment_id,
                origin: crate::embedded_audio::SessionStartOrigin::User,
            });
            observation.record_asr_queued_for_segment(Some(segment_id), 3_200);
        }

        observation.record_capture_event(&crate::embedded_audio::SessionEvent::Started {
            session_id: 99,
            origin: crate::embedded_audio::SessionStartOrigin::User,
        });

        assert_eq!(
            observation
                .segment_counters_for_test(0)
                .asr_pending_pcm_bytes,
            3_200
        );
        assert_eq!(
            observation
                .segment_counters_for_test(99)
                .capture_started_count,
            0
        );
        let state = observation.state.lock();
        assert!(state.segment_ledger_incomplete);
        assert_eq!(state.segment_ledger_overflow_count, 1);
    }

    #[test]
    fn only_settled_segments_are_evicted_after_a_final_snapshot() {
        let observation = EmbeddedAudioPipelineObservation::new(110);
        for segment_id in 0..32 {
            observation.record_capture_event(&crate::embedded_audio::SessionEvent::Started {
                session_id: segment_id,
                origin: crate::embedded_audio::SessionStartOrigin::User,
            });
            observation.record_capture_event(&crate::embedded_audio::SessionEvent::Stopped {
                session_id: segment_id,
                expected_packet_count: 0,
                origin: crate::embedded_audio::SessionStopOrigin::User,
            });
            observation.record_asr_queued_for_segment(Some(segment_id), 3_200);
            observation.record_asr_send_completed_for_segment(Some(segment_id), 3_200);
            observation.record_coordinator_terminal_drained(Some(segment_id));
            observation.snapshot("segment_settled", true);
        }

        observation.record_capture_event(&crate::embedded_audio::SessionEvent::Started {
            session_id: 99,
            origin: crate::embedded_audio::SessionStartOrigin::User,
        });

        assert_eq!(
            observation
                .segment_counters_for_test(0)
                .capture_started_count,
            0
        );
        assert_eq!(
            observation
                .segment_counters_for_test(31)
                .capture_started_count,
            1
        );
        assert_eq!(
            observation
                .segment_counters_for_test(99)
                .capture_started_count,
            1
        );
        assert!(!observation.state.lock().segment_ledger_incomplete);
    }

    #[test]
    fn source_intervals_are_checked_and_split_in_order() {
        let observation = EmbeddedAudioPipelineObservation::new(117);
        let mut first = observation
            .allocate_source_interval(700, Some(7), 2_000)
            .expect("first source interval");
        let second = observation
            .allocate_source_interval(700, Some(7), 2_400)
            .expect("second source interval");

        assert_eq!(
            first.range,
            PcmRange {
                start: 0,
                end: 2_000
            }
        );
        assert_eq!(
            second.range,
            PcmRange {
                start: 2_000,
                end: 4_400
            }
        );
        assert_eq!(first.source_stream_id, 700);
        assert_eq!(first.stream_kind, PcmStreamKind::CoordinatorInputPcm);
        assert_eq!(first.mapping, PcmMappingKind::PositionPreserving);

        let first_half = first.take_prefix(1_000).expect("first half");
        let first_tail = first.take_prefix(1_000).expect("first tail");
        let mut second = second;
        let second_prefix = second.take_prefix(2_400).expect("second source");

        assert_eq!(
            first_half.range,
            PcmRange {
                start: 0,
                end: 1_000
            }
        );
        assert_eq!(
            first_tail.range,
            PcmRange {
                start: 1_000,
                end: 2_000
            }
        );
        assert_eq!(
            second_prefix.range,
            PcmRange {
                start: 2_000,
                end: 4_400
            }
        );
    }

    #[test]
    fn source_interval_overflow_is_incomplete_without_rejecting_audio() {
        let observation = EmbeddedAudioPipelineObservation::new(118);
        observation
            .state
            .lock()
            .source_interval_offsets
            .insert((800, Some(8)), u64::MAX - 1);

        assert!(observation
            .allocate_source_interval(800, Some(8), 4)
            .is_none());
        assert!(observation
            .allocate_source_interval(800, Some(8), 1)
            .is_none());
        let state = observation.state.lock();
        assert!(state.interval_ledger_incomplete);
        assert_eq!(state.interval_ledger_overflow_count, 1);
        assert_eq!(state.interval_ledger_overflow_bytes, 4);
    }

    #[test]
    fn source_interval_mismatch_cannot_reuse_a_previous_range() {
        let observation = EmbeddedAudioPipelineObservation::new(119);
        let mut interval = observation
            .allocate_source_interval(900, Some(9), 100)
            .expect("source interval");

        assert!(interval.take_prefix(101).is_none());
        observation.mark_source_interval_incomplete();
        assert!(observation.state.lock().interval_ledger_incomplete);
        assert_eq!(
            observation
                .allocate_source_interval(900, Some(9), 100)
                .expect("next allocation is not the old interval")
                .range,
            PcmRange {
                start: 100,
                end: 200
            }
        );
    }

    #[test]
    fn source_interval_offsets_are_isolated_by_independent_stream_identity() {
        let observation = EmbeddedAudioPipelineObservation::new(220);
        let first_stream = observation
            .allocate_source_interval(2_201, Some(7), 100)
            .expect("first stream interval");
        let second_stream = observation
            .allocate_source_interval(2_202, Some(7), 100)
            .expect("second stream interval");
        assert_eq!(first_stream.range, PcmRange { start: 0, end: 100 });
        assert_eq!(second_stream.range, PcmRange { start: 0, end: 100 });
        assert_ne!(
            first_stream.source_stream_id,
            second_stream.source_stream_id
        );

        let late_first_stream = observation
            .allocate_source_interval(2_201, Some(7), 50)
            .expect("late first stream interval");
        assert_eq!(late_first_stream.source_stream_id, 2_201);
        assert_eq!(
            late_first_stream.range,
            PcmRange {
                start: 100,
                end: 150
            }
        );

        observation
            .state
            .lock()
            .source_interval_offsets
            .insert((2_201, Some(8)), u64::MAX - 1);
        assert!(observation
            .allocate_source_interval(2_201, Some(8), 4)
            .is_none());
        assert!(observation
            .allocate_source_interval(2_201, Some(8), 1)
            .is_none());
        assert_eq!(
            observation
                .allocate_source_interval(2_202, Some(8), 4)
                .expect("other stream is not poisoned by overflow")
                .range,
            PcmRange { start: 0, end: 4 }
        );
    }

    #[test]
    fn asr_destination_fact_updates_one_operation_and_is_bounded() {
        let observation = EmbeddedAudioPipelineObservation::new(223);
        let share = AsrSourceShareFact {
            segment_id: Some(7),
            bytes: 4,
            destination_range: Some(PcmRange { start: 0, end: 4 }),
            source_interval: None,
            collector_metadata: None,
            collector_emitted_range: None,
        };
        let fact = |outcome| AsrDestinationFact {
            asr_stream_id: 2_230,
            sequence: Some(1),
            destination_range: Some(PcmRange { start: 0, end: 4 }),
            outcome,
            source_shares: vec![share],
        };
        observation.record_asr_destination_fact(fact(AsrDestinationOutcome::Queued));
        observation.record_asr_destination_fact(fact(AsrDestinationOutcome::Claimed));
        observation.record_asr_destination_fact(fact(AsrDestinationOutcome::SocketSendCompleted));
        let facts = observation.asr_destination_facts_for_test();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].outcome, AsrDestinationOutcome::SocketSendCompleted);
        assert_eq!(facts[0].source_shares, vec![share]);

        observation.record_asr_destination_fact(AsrDestinationFact {
            asr_stream_id: 2_230,
            sequence: Some(2),
            destination_range: Some(PcmRange { start: 2, end: 3 }),
            outcome: AsrDestinationOutcome::Queued,
            source_shares: vec![share; ASR_DESTINATION_SHARES_PER_FACT_CAPACITY + 1],
        });
        let truncated = observation.asr_destination_facts_for_test();
        assert_eq!(
            truncated[1].source_shares.len(),
            ASR_DESTINATION_SHARES_PER_FACT_CAPACITY
        );
        assert!(observation.asr_destination_capacity_drops_for_test().4);

        for sequence in 3..=(ASR_DESTINATION_FACT_CAPACITY as i32 + 1) {
            observation.record_asr_destination_fact(AsrDestinationFact {
                asr_stream_id: 2_230,
                sequence: Some(sequence),
                destination_range: Some(PcmRange {
                    start: sequence as u64,
                    end: sequence as u64 + 1,
                }),
                outcome: AsrDestinationOutcome::Queued,
                source_shares: Vec::new(),
            });
        }
        assert_eq!(
            observation.asr_destination_facts_for_test().len(),
            ASR_DESTINATION_FACT_CAPACITY
        );
        assert_eq!(
            observation.asr_destination_capacity_drops_for_test(),
            (1, 0, 1, 4, true)
        );
    }

    #[test]
    fn asr_destination_fact_key_and_terminal_merge_are_monotonic() {
        let observation = EmbeddedAudioPipelineObservation::new(224);
        let share = AsrSourceShareFact {
            segment_id: Some(8),
            bytes: 4,
            destination_range: Some(PcmRange { start: 0, end: 4 }),
            source_interval: None,
            collector_metadata: None,
            collector_emitted_range: None,
        };
        let make = |destination_range, outcome, source_shares| AsrDestinationFact {
            asr_stream_id: 2_240,
            sequence: Some(3),
            destination_range,
            outcome,
            source_shares,
        };

        observation.record_asr_destination_fact(make(
            Some(PcmRange { start: 0, end: 4 }),
            AsrDestinationOutcome::Queued,
            vec![share],
        ));
        // The destination range is evidence, not identity. A changed range
        // cannot manufacture a second operation and is surfaced as
        // incomplete metadata.
        observation.record_asr_destination_fact(make(
            Some(PcmRange { start: 4, end: 8 }),
            AsrDestinationOutcome::Claimed,
            vec![share],
        ));
        observation.record_asr_destination_fact(make(
            Some(PcmRange { start: 0, end: 4 }),
            AsrDestinationOutcome::SocketSendCompleted,
            vec![share],
        ));
        // Repeating the same terminal observation is idempotent.
        observation.record_asr_destination_fact(make(
            Some(PcmRange { start: 0, end: 4 }),
            AsrDestinationOutcome::SocketSendCompleted,
            vec![share],
        ));
        // A later callback may lose metadata, but it must leave an explicit
        // missing-data mark rather than making the operation look complete at
        // every lifecycle point.
        observation.record_asr_destination_fact(make(
            None,
            AsrDestinationOutcome::SocketSendCompleted,
            Vec::new(),
        ));
        // A late queued callback cannot downgrade a completed operation.
        observation.record_asr_destination_fact(make(
            Some(PcmRange { start: 0, end: 4 }),
            AsrDestinationOutcome::Queued,
            vec![share],
        ));
        let facts = observation.asr_destination_facts_for_test();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].outcome, AsrDestinationOutcome::SocketSendCompleted);
        assert_eq!(
            observation.asr_destination_fact_conflicts_for_test(),
            (2, 8)
        );
        assert_eq!(
            observation.asr_destination_fact_missing_metadata_for_test(),
            (1, 0)
        );
        assert!(observation.asr_destination_capacity_drops_for_test().4);

        // Distinct terminal outcomes for one sequenced operation are not
        // resolved by callback order; preserve the contradiction explicitly.
        let conflicting = |outcome| AsrDestinationFact {
            asr_stream_id: 2_240,
            sequence: Some(4),
            destination_range: Some(PcmRange { start: 8, end: 12 }),
            outcome,
            source_shares: vec![share],
        };
        observation
            .record_asr_destination_fact(conflicting(AsrDestinationOutcome::SocketSendCompleted));
        observation.record_asr_destination_fact(conflicting(AsrDestinationOutcome::SendFailed));
        assert_eq!(
            observation
                .asr_destination_facts_for_test()
                .iter()
                .find(|fact| fact.sequence == Some(4))
                .expect("conflicting operation")
                .outcome,
            AsrDestinationOutcome::Conflict
        );
        assert_eq!(
            observation.asr_destination_fact_conflicts_for_test(),
            (3, 12)
        );

        // Sequence-less buffered tails do not have an operation key; keep
        // them separate instead of silently merging unrelated tails.
        observation.record_asr_destination_fact(AsrDestinationFact {
            asr_stream_id: 2_240,
            sequence: None,
            destination_range: None,
            outcome: AsrDestinationOutcome::BufferedUnresolved,
            source_shares: vec![share],
        });
        observation.record_asr_destination_fact(AsrDestinationFact {
            asr_stream_id: 2_240,
            sequence: None,
            destination_range: None,
            outcome: AsrDestinationOutcome::BufferedUnresolved,
            source_shares: vec![share],
        });
        assert_eq!(observation.asr_destination_facts_for_test().len(), 4);
    }

    #[test]
    fn pcm_stage_mapping_is_bounded_and_keeps_session_identity() {
        let observation = EmbeddedAudioPipelineObservation::new(229);
        let share = AsrSourceShareFact {
            segment_id: Some(9),
            bytes: 4,
            destination_range: Some(PcmRange { start: 0, end: 4 }),
            source_interval: None,
            collector_metadata: None,
            collector_emitted_range: None,
        };
        let fact = |logical_stream_id| PcmStageMappingFact {
            logical_stream_id,
            operation_id: Some(logical_stream_id),
            stage: PcmStage::Normalized,
            stream_kind: PcmStreamKind::CoordinatorInputPcm,
            pcm_format: PcmFormat::PcmS16LeMono16k,
            mapping: PcmMappingKind::PositionPreserving,
            disposition: PcmStageDisposition::Appended,
            destination_range: Some(PcmRange { start: 0, end: 4 }),
            source_shares: vec![share],
        };
        observation.record_pcm_stage_mapping(fact(2_290));
        observation.record_pcm_stage_mapping(fact(2_291));
        assert_eq!(
            observation
                .pcm_stage_facts_for_test()
                .iter()
                .map(|fact| fact.logical_stream_id)
                .collect::<Vec<_>>(),
            vec![2_290, 2_291]
        );

        let oversized = PcmStageMappingFact {
            logical_stream_id: 2_292,
            operation_id: Some(2_292),
            stage: PcmStage::Archive,
            stream_kind: PcmStreamKind::CoordinatorInputPcm,
            pcm_format: PcmFormat::PcmS16LeMono16k,
            mapping: PcmMappingKind::PositionPreserving,
            disposition: PcmStageDisposition::Appended,
            destination_range: Some(PcmRange { start: 4, end: 8 }),
            source_shares: vec![share; PCM_STAGE_SHARES_PER_FACT_CAPACITY + 1],
        };
        observation.record_pcm_stage_mapping(oversized);
        for _ in 0..(PCM_STAGE_FACT_CAPACITY - 2) {
            observation.record_pcm_stage_mapping(fact(2_293));
        }
        assert_eq!(
            observation.pcm_stage_facts_for_test().len(),
            PCM_STAGE_FACT_CAPACITY
        );
        assert_eq!(
            observation.pcm_stage_capacity_drops_for_test(),
            (1, 4, 1, 4, true)
        );
    }

    #[test]
    fn disabled_pcm_stage_does_not_make_ledger_incomplete() {
        let mut ledger = PcmStageMappingLedger::default();
        ledger.record(PcmStageMappingFact {
            logical_stream_id: 2_294,
            operation_id: Some(1),
            stage: PcmStage::Archive,
            stream_kind: PcmStreamKind::CoordinatorInputPcm,
            pcm_format: PcmFormat::PcmS16LeMono16k,
            mapping: PcmMappingKind::Unknown,
            disposition: PcmStageDisposition::Disabled,
            destination_range: None,
            source_shares: vec![AsrSourceShareFact {
                segment_id: None,
                bytes: 0,
                destination_range: None,
                source_interval: None,
                collector_metadata: None,
                collector_emitted_range: None,
            }],
        });
        assert!(!ledger.capacity_drops().4);

        ledger.record(PcmStageMappingFact {
            logical_stream_id: 2_294,
            operation_id: Some(2),
            stage: PcmStage::Normalized,
            stream_kind: PcmStreamKind::CoordinatorInputPcm,
            pcm_format: PcmFormat::PcmS16LeMono16k,
            mapping: PcmMappingKind::Unknown,
            disposition: PcmStageDisposition::Appended,
            destination_range: None,
            source_shares: Vec::new(),
        });
        assert!(ledger.capacity_drops().4);
    }

    #[test]
    fn pcm_range_and_mapping_facts_are_checked_without_claiming_content_identity() {
        assert_eq!(
            PcmRange::from_start_and_bytes(100, 25),
            Some(PcmRange {
                start: 100,
                end: 125
            })
        );
        assert!(PcmRange::from_start_and_bytes(u64::MAX, 1).is_none());
        assert_eq!(
            serde_json::to_value(PcmMappingKind::PositionPreserving).expect("mapping JSON"),
            serde_json::json!("position_preserving")
        );
        assert_eq!(
            serde_json::to_value(PcmMappingKind::Unknown).expect("mapping JSON"),
            serde_json::json!("unknown")
        );
    }
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

// Evidence-only counters for locating a missing embedded-audio stage.  These
// counters deliberately live beside, rather than inside, the audio protocol
// state machine: a logging failure or a snapshot race must never change packet
// admission, recovery, endpointing, or delivery behavior.
#[derive(Clone, Copy, Default, Serialize)]
struct PipelineCounters {
    raw_notify_count: u64,
    raw_notify_bytes: u64,
    raw_notify_unknown_type_count: u64,
    channel_forward_success_count: u64,
    channel_forward_failure_count: u64,
    coordinator_channel_forward_success_count: u64,
    coordinator_channel_forward_failure_count: u64,
    normalize_success_count: u64,
    normalize_reject_count: u64,
    normalize_input_bytes: u64,
    normalize_output_bytes: u64,
    capture_started_count: u64,
    capture_audio_count: u64,
    capture_audio_payload_bytes: u64,
    capture_ignored_count: u64,
    capture_ignored_foreign_count: u64,
    capture_ignored_duplicate_count: u64,
    capture_stopped_count: u64,
    capture_cancelled_count: u64,
    capture_error_count: u64,
    capture_parse_reject_count: u64,
    coordinator_channel_received_count: u64,
    coordinator_channel_received_bytes: u64,
    coordinator_started_count: u64,
    coordinator_pcm_chunk_count: u64,
    coordinator_pcm_bytes: u64,
    coordinator_stopped_count: u64,
    coordinator_cancelled_count: u64,
    coordinator_error_count: u64,
    coordinator_terminal_drained_count: u64,
    coordinator_ignored_count: u64,
    coordinator_parse_reject_count: u64,
    consumer_accepted_chunk_count: u64,
    consumer_accepted_pcm_bytes: u64,
    consumer_rejected_chunk_count: u64,
    consumer_rejected_pcm_bytes: u64,
    asr_queued_frame_count: u64,
    asr_queued_pcm_bytes: u64,
    asr_send_completed_frame_count: u64,
    asr_send_completed_pcm_bytes: u64,
    asr_send_failed_frame_count: u64,
    asr_send_failed_pcm_bytes: u64,
    asr_pending_frame_count: u64,
    asr_pending_pcm_bytes: u64,
    asr_queue_rejected_frame_count: u64,
    asr_queue_rejected_pcm_bytes: u64,
    asr_abandoned_frame_count: u64,
    asr_abandoned_pcm_bytes: u64,
    asr_unresolved_chunk_count: u64,
    asr_unresolved_pcm_bytes: u64,
    unknown_source_event_count: u64,
    unknown_source_pcm_bytes: u64,
}

fn segment_delivery_settled(counters: &PipelineCounters) -> bool {
    counters.asr_pending_frame_count == 0 && counters.asr_pending_pcm_bytes == 0
}

fn segment_counters_settled(counters: &PipelineCounters) -> bool {
    (counters.capture_stopped_count > 0
        || counters.capture_cancelled_count > 0
        || counters.capture_error_count > 0
        || counters.coordinator_stopped_count > 0
        || counters.coordinator_cancelled_count > 0
        || counters.coordinator_error_count > 0)
        && counters.coordinator_terminal_drained_count > 0
        && segment_delivery_settled(counters)
}

fn ensure_segment_counter(
    state: &mut PipelineObservationState,
    segment_id: u32,
    overflow_bytes: u64,
) -> bool {
    if !state.segment_counters.contains_key(&segment_id) && state.segment_counters.len() >= 32 {
        let settled = state
            .segment_final_snapshot_emitted
            .iter()
            .copied()
            .find(|candidate| {
                state
                    .segment_counters
                    .get(candidate)
                    .is_some_and(segment_counters_settled)
            });
        if let Some(settled) = settled {
            state.segment_counters.remove(&settled);
            state.segment_final_snapshot_emitted.remove(&settled);
        }
    }
    if state.segment_counters.len() >= 32 && !state.segment_counters.contains_key(&segment_id) {
        state.segment_ledger_overflow_count = state.segment_ledger_overflow_count.saturating_add(1);
        state.segment_ledger_overflow_bytes = state
            .segment_ledger_overflow_bytes
            .saturating_add(overflow_bytes);
        state.segment_ledger_incomplete = true;
        return false;
    }
    state.segment_counters.entry(segment_id).or_default();
    true
}

struct PipelineObservationState {
    started_at: Instant,
    snapshot_seq: u64,
    last_snapshot_at: Option<Instant>,
    capture_ended: bool,
    coordinator_session_id: Option<String>,
    embedded_session_id: Option<u32>,
    counters: PipelineCounters,
    segment_counters: BTreeMap<u32, PipelineCounters>,
    segment_ledger_overflow_count: u64,
    segment_ledger_overflow_bytes: u64,
    segment_ledger_incomplete: bool,
    segment_final_snapshot_emitted: BTreeSet<u32>,
    source_interval_offsets: BTreeMap<(u64, Option<u32>), u64>,
    source_interval_unknown_segments: BTreeSet<(u64, Option<u32>)>,
    interval_ledger_overflow_count: u64,
    interval_ledger_overflow_bytes: u64,
    interval_ledger_incomplete: bool,
    asr_destination_facts: VecDeque<AsrDestinationFact>,
    // These counters describe bounded-observation loss, not PCM loss. An
    // update is counted each time a new fact cannot enter the ledger.
    asr_destination_fact_update_drop_count: u64,
    asr_destination_fact_update_drop_bytes: u64,
    // Shares beyond one operation's projection cap are counted separately;
    // mixed-source facts remain local projections, never a PCM-loss total.
    asr_destination_share_drop_count: u64,
    asr_destination_share_drop_bytes: u64,
    // Existing trusted evidence followed by missing evidence is retained but
    // marked incomplete so the projection cannot be mistaken for a full trace.
    asr_destination_fact_missing_metadata_count: u64,
    asr_destination_fact_missing_metadata_bytes: u64,
    asr_destination_fact_conflict_count: u64,
    asr_destination_fact_conflict_bytes: u64,
    asr_destination_facts_incomplete: bool,
    pcm_stage_ledger: PcmStageMappingLedger,
    collector_pcm_facts: VecDeque<CollectorPcmChunkFact>,
    collector_pcm_fact_drop_count: u64,
    collector_pcm_fact_drop_bytes: u64,
    collector_pcm_facts_incomplete: bool,
    candidate_ledger: CandidateFactLedger,
}

pub(crate) struct EmbeddedAudioPipelineObservation {
    capture_generation: u64,
    state: Mutex<PipelineObservationState>,
    snapshot_sink: Mutex<Option<Arc<dyn Fn(String) + Send + Sync>>>,
}

impl EmbeddedAudioPipelineObservation {
    fn new(capture_generation: u64) -> Self {
        Self {
            capture_generation,
            state: Mutex::new(PipelineObservationState {
                started_at: Instant::now(),
                snapshot_seq: 0,
                last_snapshot_at: None,
                capture_ended: false,
                coordinator_session_id: None,
                embedded_session_id: None,
                counters: PipelineCounters::default(),
                segment_counters: BTreeMap::new(),
                segment_ledger_overflow_count: 0,
                segment_ledger_overflow_bytes: 0,
                segment_ledger_incomplete: false,
                segment_final_snapshot_emitted: BTreeSet::new(),
                source_interval_offsets: BTreeMap::new(),
                source_interval_unknown_segments: BTreeSet::new(),
                interval_ledger_overflow_count: 0,
                interval_ledger_overflow_bytes: 0,
                interval_ledger_incomplete: false,
                asr_destination_facts: VecDeque::new(),
                asr_destination_fact_update_drop_count: 0,
                asr_destination_fact_update_drop_bytes: 0,
                asr_destination_share_drop_count: 0,
                asr_destination_share_drop_bytes: 0,
                asr_destination_fact_missing_metadata_count: 0,
                asr_destination_fact_missing_metadata_bytes: 0,
                asr_destination_fact_conflict_count: 0,
                asr_destination_fact_conflict_bytes: 0,
                asr_destination_facts_incomplete: false,
                pcm_stage_ledger: PcmStageMappingLedger::default(),
                collector_pcm_facts: VecDeque::new(),
                collector_pcm_fact_drop_count: 0,
                collector_pcm_fact_drop_bytes: 0,
                collector_pcm_facts_incomplete: false,
                candidate_ledger: CandidateFactLedger::default(),
            }),
            snapshot_sink: Mutex::new(None),
        }
    }

    #[cfg(test)]
    fn set_snapshot_sink_for_test(&self, sink: Arc<dyn Fn(String) + Send + Sync>) {
        *self.snapshot_sink.lock() = Some(sink);
    }

    fn mutate<F>(&self, update: F)
    where
        F: FnOnce(&mut PipelineObservationState),
    {
        update(&mut self.state.lock());
    }

    fn mutate_counters_for_segment<F>(&self, segment_id: Option<u32>, mut update: F)
    where
        F: FnMut(&mut PipelineCounters),
    {
        self.mutate(|state| {
            update(&mut state.counters);
            if let Some(segment_id) = segment_id {
                if ensure_segment_counter(state, segment_id, 0) {
                    update(
                        state
                            .segment_counters
                            .get_mut(&segment_id)
                            .expect("segment inserted"),
                    );
                }
            } else {
                state.counters.unknown_source_event_count =
                    state.counters.unknown_source_event_count.saturating_add(1);
            }
        });
    }

    fn mutate_active_counters<F>(&self, update: F)
    where
        F: FnMut(&mut PipelineCounters),
    {
        let active_segment = self.state.lock().embedded_session_id;
        self.mutate_counters_for_segment(active_segment, update);
    }

    pub(crate) fn capture_generation(&self) -> u64 {
        self.capture_generation
    }

    pub(crate) fn bind_sessions(
        &self,
        coordinator_session_id: Option<SessionId>,
        embedded_session_id: Option<u32>,
    ) {
        self.mutate(|state| {
            if let Some(session_id) = coordinator_session_id {
                state.coordinator_session_id = Some(session_id.to_string());
            }
            if let Some(session_id) = embedded_session_id {
                state.embedded_session_id = Some(session_id);
            }
        });
    }

    /// Allocates a source coordinate exactly once at the first clear PCM
    /// boundary. The coordinate is scoped by the logical coordinator stream
    /// and segment; it is not a firmware sample offset claim.
    pub(crate) fn allocate_source_interval(
        &self,
        source_stream_id: u64,
        segment_id: Option<u32>,
        bytes: usize,
    ) -> Option<PcmSourceInterval> {
        if bytes == 0 {
            return None;
        }
        let mut state = self.state.lock();
        let stream_segment = (source_stream_id, segment_id);
        if source_stream_id == 0 {
            state
                .source_interval_unknown_segments
                .insert(stream_segment);
            state.interval_ledger_incomplete = true;
            return None;
        }
        if state
            .source_interval_unknown_segments
            .contains(&stream_segment)
        {
            state.interval_ledger_incomplete = true;
            return None;
        }
        let bytes_u64 = match u64::try_from(bytes) {
            Ok(bytes) => bytes,
            Err(_) => {
                state
                    .source_interval_unknown_segments
                    .insert(stream_segment);
                state.interval_ledger_overflow_count =
                    state.interval_ledger_overflow_count.saturating_add(1);
                state.interval_ledger_overflow_bytes = state
                    .interval_ledger_overflow_bytes
                    .saturating_add(u64::MAX);
                state.interval_ledger_incomplete = true;
                return None;
            }
        };
        let start = state
            .source_interval_offsets
            .get(&stream_segment)
            .copied()
            .unwrap_or_default();
        let Some(end) = start.checked_add(bytes_u64) else {
            state.interval_ledger_overflow_count =
                state.interval_ledger_overflow_count.saturating_add(1);
            state.interval_ledger_overflow_bytes = state
                .interval_ledger_overflow_bytes
                .saturating_add(bytes_u64);
            state
                .source_interval_unknown_segments
                .insert(stream_segment);
            state.interval_ledger_incomplete = true;
            return None;
        };
        state.source_interval_offsets.insert(stream_segment, end);
        Some(PcmSourceInterval {
            capture_generation: self.capture_generation,
            source_stream_id,
            stream_kind: PcmStreamKind::CoordinatorInputPcm,
            mapping: PcmMappingKind::PositionPreserving,
            segment_id,
            range: PcmRange { start, end },
        })
    }

    pub(crate) fn mark_source_interval_incomplete(&self) {
        self.mutate(|state| {
            state.interval_ledger_incomplete = true;
        });
    }

    #[cfg(test)]
    pub(crate) fn interval_ledger_incomplete_for_test(&self) -> bool {
        self.state.lock().interval_ledger_incomplete
    }

    pub(crate) fn active_embedded_session_id(&self) -> Option<u32> {
        self.state.lock().embedded_session_id
    }

    pub(crate) fn record_raw_notification(&self, bytes: usize) {
        self.mutate(|state| {
            state.counters.raw_notify_count += 1;
            state.counters.raw_notify_bytes =
                state.counters.raw_notify_bytes.saturating_add(bytes as u64);
            // The callback intentionally does not parse the packet.  Unknown
            // packet types are therefore classified only after normalization;
            // this raw-stage counter remains zero unless a future callback
            // parser can prove that fact without duplicating protocol logic.
        });
    }

    pub(crate) fn record_channel_forward(&self, success: bool) {
        self.mutate(|state| {
            if success {
                state.counters.channel_forward_success_count += 1;
            } else {
                state.counters.channel_forward_failure_count += 1;
            }
        });
    }

    pub(crate) fn record_coordinator_channel_forward(&self, success: bool) {
        self.mutate(|state| {
            if success {
                state.counters.coordinator_channel_forward_success_count += 1;
            } else {
                state.counters.coordinator_channel_forward_failure_count += 1;
            }
        });
    }

    pub(crate) fn record_normalize(&self, input_bytes: usize, output_bytes: Option<usize>) {
        self.mutate(|state| {
            state.counters.normalize_input_bytes = state
                .counters
                .normalize_input_bytes
                .saturating_add(input_bytes as u64);
            match output_bytes {
                Some(output_bytes) => {
                    state.counters.normalize_success_count += 1;
                    state.counters.normalize_output_bytes = state
                        .counters
                        .normalize_output_bytes
                        .saturating_add(output_bytes as u64);
                }
                None => state.counters.normalize_reject_count += 1,
            }
        });
    }

    pub(crate) fn record_capture_parse_reject(&self) {
        self.mutate(|state| state.counters.capture_parse_reject_count += 1);
    }

    pub(crate) fn record_capture_event(&self, event: &crate::embedded_audio::SessionEvent) {
        let snapshot_event = match event {
            crate::embedded_audio::SessionEvent::Started { session_id, .. } => {
                self.mutate(|state| state.embedded_session_id = Some(*session_id));
                self.mutate_counters_for_segment(Some(*session_id), |c| {
                    c.capture_started_count += 1;
                });
                "capture_started"
            }
            crate::embedded_audio::SessionEvent::AudioData {
                session_id,
                pcm_bytes,
                ..
            } => {
                self.mutate(|state| state.embedded_session_id = Some(*session_id));
                self.mutate_counters_for_segment(Some(*session_id), |c| {
                    c.capture_audio_count += 1;
                    c.capture_audio_payload_bytes = c
                        .capture_audio_payload_bytes
                        .saturating_add(*pcm_bytes as u64);
                });
                "capture_audio"
            }
            crate::embedded_audio::SessionEvent::Stopped { session_id, .. } => {
                self.mutate(|state| state.embedded_session_id = Some(*session_id));
                self.mutate_counters_for_segment(Some(*session_id), |c| {
                    c.capture_stopped_count += 1;
                });
                "capture_stopped"
            }
            crate::embedded_audio::SessionEvent::Cancelled { session_id, .. } => {
                self.mutate(|state| state.embedded_session_id = Some(*session_id));
                self.mutate_counters_for_segment(Some(*session_id), |c| {
                    c.capture_cancelled_count += 1;
                });
                "capture_cancelled"
            }
            crate::embedded_audio::SessionEvent::Error { session_id, .. } => {
                self.mutate(|state| state.embedded_session_id = Some(*session_id));
                self.mutate_counters_for_segment(Some(*session_id), |c| {
                    c.capture_error_count += 1;
                });
                "capture_error"
            }
            crate::embedded_audio::SessionEvent::Ignored(reason) => {
                self.mutate(|state| {
                    state.counters.capture_ignored_count += 1;
                    match reason {
                        crate::embedded_audio::IgnoredPacketReason::ForeignSession => {
                            state.counters.capture_ignored_foreign_count += 1
                        }
                        crate::embedded_audio::IgnoredPacketReason::DuplicateOrShorterPacket => {
                            state.counters.capture_ignored_duplicate_count += 1
                        }
                        _ => {}
                    }
                });
                "capture_ignored"
            }
        };
        if !matches!(event, crate::embedded_audio::SessionEvent::AudioData { .. }) {
            self.snapshot(snapshot_event, true);
        }
    }

    pub(crate) fn record_coordinator_channel_received(&self, bytes: usize) {
        self.mutate(|state| {
            state.counters.coordinator_channel_received_count += 1;
            state.counters.coordinator_channel_received_bytes = state
                .counters
                .coordinator_channel_received_bytes
                .saturating_add(bytes as u64);
        });
    }

    pub(crate) fn record_coordinator_parse_reject(&self) {
        self.mutate(|state| state.counters.coordinator_parse_reject_count += 1);
        self.snapshot("coordinator_parse_reject", true);
    }

    pub(crate) fn record_coordinator_event(
        &self,
        event: &crate::embedded_audio::StreamingSessionEvent,
    ) {
        let snapshot_event = match event {
            crate::embedded_audio::StreamingSessionEvent::Started { session_id, .. } => {
                self.mutate(|state| state.embedded_session_id = Some(*session_id));
                self.mutate_counters_for_segment(Some(*session_id), |c| {
                    c.coordinator_started_count += 1;
                });
                "coordinator_started"
            }
            crate::embedded_audio::StreamingSessionEvent::PcmChunk(chunk) => {
                self.mutate(|state| state.embedded_session_id = Some(chunk.session_id));
                self.mutate(|state| {
                    if let Some(metadata) = chunk.metadata {
                        if metadata.metadata_incomplete {
                            state.collector_pcm_facts_incomplete = true;
                        }
                        let fact = CollectorPcmChunkFact {
                            session_id: chunk.session_id,
                            packet_sequence: chunk.packet_sequence,
                            pcm_bytes: chunk.pcm.len() as u64,
                            metadata,
                        };
                        if state.collector_pcm_facts.len() >= COLLECTOR_PCM_FACT_CAPACITY {
                            state.collector_pcm_fact_drop_count =
                                state.collector_pcm_fact_drop_count.saturating_add(1);
                            state.collector_pcm_fact_drop_bytes = state
                                .collector_pcm_fact_drop_bytes
                                .saturating_add(fact.pcm_bytes);
                            state.collector_pcm_facts_incomplete = true;
                        } else {
                            state.collector_pcm_facts.push_back(fact);
                        }
                    } else {
                        state.collector_pcm_facts_incomplete = true;
                    }
                });
                self.mutate_counters_for_segment(Some(chunk.session_id), |c| {
                    c.coordinator_pcm_chunk_count += 1;
                    c.coordinator_pcm_bytes = c
                        .coordinator_pcm_bytes
                        .saturating_add(chunk.pcm.len() as u64);
                });
                "coordinator_pcm"
            }
            crate::embedded_audio::StreamingSessionEvent::Stopped { session_id, .. } => {
                self.mutate(|state| state.embedded_session_id = Some(*session_id));
                self.mutate_counters_for_segment(Some(*session_id), |c| {
                    c.coordinator_stopped_count += 1;
                });
                "coordinator_stopped"
            }
            crate::embedded_audio::StreamingSessionEvent::Cancelled { session_id, .. } => {
                self.mutate(|state| state.embedded_session_id = Some(*session_id));
                self.mutate_counters_for_segment(Some(*session_id), |c| {
                    c.coordinator_cancelled_count += 1;
                });
                "coordinator_cancelled"
            }
            crate::embedded_audio::StreamingSessionEvent::Error { session_id, .. } => {
                self.mutate(|state| state.embedded_session_id = Some(*session_id));
                self.mutate_counters_for_segment(Some(*session_id), |c| {
                    c.coordinator_error_count += 1;
                });
                "coordinator_error"
            }
            crate::embedded_audio::StreamingSessionEvent::Ignored(_) => {
                self.mutate(|state| state.counters.coordinator_ignored_count += 1);
                "coordinator_ignored"
            }
        };
        if !matches!(
            event,
            crate::embedded_audio::StreamingSessionEvent::PcmChunk(_)
        ) {
            self.snapshot(snapshot_event, true);
        }
    }

    pub(crate) fn record_coordinator_terminal_drained(&self, segment_id: Option<u32>) {
        let Some(segment_id) = segment_id else {
            return;
        };
        self.mutate_counters_for_segment(Some(segment_id), |c| {
            c.coordinator_terminal_drained_count =
                c.coordinator_terminal_drained_count.saturating_add(1);
        });
        self.snapshot("coordinator_terminal_drained", true);
    }

    pub(crate) fn record_consumer_result(&self, accepted: bool, bytes: usize) {
        self.mutate_active_counters(|c| {
            if accepted {
                c.consumer_accepted_chunk_count += 1;
                c.consumer_accepted_pcm_bytes =
                    c.consumer_accepted_pcm_bytes.saturating_add(bytes as u64);
            } else {
                c.consumer_rejected_chunk_count += 1;
                c.consumer_rejected_pcm_bytes =
                    c.consumer_rejected_pcm_bytes.saturating_add(bytes as u64);
            }
        });
    }

    pub(crate) fn record_consumer_result_for_segment(
        &self,
        segment_id: Option<u32>,
        accepted: bool,
        bytes: usize,
    ) {
        self.mutate_counters_for_segment(segment_id, |c| {
            if accepted {
                c.consumer_accepted_chunk_count += 1;
                c.consumer_accepted_pcm_bytes =
                    c.consumer_accepted_pcm_bytes.saturating_add(bytes as u64);
            } else {
                c.consumer_rejected_chunk_count += 1;
                c.consumer_rejected_pcm_bytes =
                    c.consumer_rejected_pcm_bytes.saturating_add(bytes as u64);
            }
        });
        if segment_id.is_none() {
            self.mutate(|state| {
                state.counters.unknown_source_pcm_bytes = state
                    .counters
                    .unknown_source_pcm_bytes
                    .saturating_add(bytes as u64);
            });
        }
    }

    /// Records one bounded lifecycle fact for an ASR destination operation.
    /// A sequenced operation is keyed only by ASR stream identity plus queue
    /// sequence. Destination range is evidence carried by the operation, not
    /// part of its identity: a changed range must be visible as incomplete or
    /// conflicting metadata rather than becoming a second operation. A
    /// sequence-less buffered tail has no stable operation key, so each such
    /// observation remains a separate fact and can never silently merge.
    pub(crate) fn record_asr_destination_fact(&self, mut fact: AsrDestinationFact) {
        let fact_bytes = fact
            .source_shares
            .iter()
            .fold(0_u64, |total, share| total.saturating_add(share.bytes));
        let dropped_share_count =
            fact.source_shares
                .len()
                .saturating_sub(ASR_DESTINATION_SHARES_PER_FACT_CAPACITY) as u64;
        let dropped_share_bytes = fact
            .source_shares
            .iter()
            .skip(ASR_DESTINATION_SHARES_PER_FACT_CAPACITY)
            .fold(0_u64, |total, share| total.saturating_add(share.bytes));
        self.mutate(|state| {
            if dropped_share_count > 0 {
                fact.source_shares
                    .truncate(ASR_DESTINATION_SHARES_PER_FACT_CAPACITY);
                state.asr_destination_share_drop_count = state
                    .asr_destination_share_drop_count
                    .saturating_add(dropped_share_count);
                state.asr_destination_share_drop_bytes = state
                    .asr_destination_share_drop_bytes
                    .saturating_add(dropped_share_bytes);
                state.asr_destination_facts_incomplete = true;
            }

            let existing_index = fact.sequence.and_then(|sequence| {
                state.asr_destination_facts.iter().position(|existing| {
                    existing.asr_stream_id == fact.asr_stream_id
                        && existing.sequence == Some(sequence)
                })
            });
            if let Some(existing_index) = existing_index {
                let (shape_conflict, missing_metadata, current_outcome) = {
                    let existing = &state.asr_destination_facts[existing_index];
                    let destination_conflict = existing.destination_range.is_some()
                        && fact.destination_range.is_some()
                        && existing.destination_range != fact.destination_range;
                    let source_conflict = !existing.source_shares.is_empty()
                        && !fact.source_shares.is_empty()
                        && existing.source_shares != fact.source_shares;
                    let destination_missing =
                        existing.destination_range.is_some() && fact.destination_range.is_none();
                    let sources_missing =
                        !existing.source_shares.is_empty() && fact.source_shares.is_empty();
                    (
                        destination_conflict || source_conflict,
                        destination_missing || sources_missing,
                        existing.outcome,
                    )
                };
                if shape_conflict {
                    state.asr_destination_fact_conflict_count =
                        state.asr_destination_fact_conflict_count.saturating_add(1);
                    state.asr_destination_fact_conflict_bytes = state
                        .asr_destination_fact_conflict_bytes
                        .saturating_add(fact_bytes);
                    state.asr_destination_facts_incomplete = true;
                }
                if missing_metadata {
                    state.asr_destination_fact_missing_metadata_count = state
                        .asr_destination_fact_missing_metadata_count
                        .saturating_add(1);
                    state.asr_destination_fact_missing_metadata_bytes = state
                        .asr_destination_fact_missing_metadata_bytes
                        .saturating_add(fact_bytes);
                    state.asr_destination_facts_incomplete = true;
                }
                let (merged_outcome, lifecycle_conflict) =
                    merge_asr_destination_outcome(current_outcome, fact.outcome);
                if lifecycle_conflict {
                    state.asr_destination_fact_conflict_count =
                        state.asr_destination_fact_conflict_count.saturating_add(1);
                    state.asr_destination_fact_conflict_bytes = state
                        .asr_destination_fact_conflict_bytes
                        .saturating_add(fact_bytes);
                    state.asr_destination_facts_incomplete = true;
                }
                let existing = &mut state.asr_destination_facts[existing_index];
                existing.outcome = merged_outcome;
                if existing.destination_range.is_none() {
                    existing.destination_range = fact.destination_range;
                }
                if existing.source_shares.is_empty() && !fact.source_shares.is_empty() {
                    existing.source_shares = fact.source_shares;
                }
                return;
            }
            if state.asr_destination_facts.len() >= ASR_DESTINATION_FACT_CAPACITY {
                // These are dropped fact *updates*, not dropped audio frames:
                // repeated lifecycle callbacks for one full queue can each
                // consume a bounded update slot. Report that exact unit so a
                // reader does not mistake this counter for PCM loss.
                state.asr_destination_fact_update_drop_count = state
                    .asr_destination_fact_update_drop_count
                    .saturating_add(1);
                state.asr_destination_fact_update_drop_bytes = state
                    .asr_destination_fact_update_drop_bytes
                    .saturating_add(fact_bytes);
                state.asr_destination_facts_incomplete = true;
                return;
            }
            state.asr_destination_facts.push_back(fact);
        });
    }

    pub(crate) fn mark_asr_destination_facts_incomplete(&self) {
        self.mutate(|state| state.asr_destination_facts_incomplete = true);
    }

    /// Records a bounded mapping projection for one coordinator pipeline
    /// stage. The projection is owned by the observation that supplied the
    /// source run; a mixed operation therefore appears as one local fact per
    /// observation rather than inventing a cross-observation owner.
    pub(crate) fn record_pcm_stage_mapping(&self, fact: PcmStageMappingFact) {
        self.mutate(|state| state.pcm_stage_ledger.record(fact));
    }

    pub(crate) fn mark_pcm_stage_facts_incomplete(&self) {
        self.mutate(|state| state.pcm_stage_ledger.mark_incomplete());
    }

    pub(crate) fn record_candidate_fact(&self, fact: CandidateFact) {
        self.mutate(|state| state.candidate_ledger.record(fact));
    }

    pub(crate) fn record_asr_queued_for_segment(&self, segment_id: Option<u32>, bytes: usize) {
        self.record_asr_queued_for_sources(&[(segment_id, bytes)], bytes);
    }

    pub(crate) fn record_asr_queued_for_sources(
        &self,
        shares: &[(Option<u32>, usize)],
        frame_bytes: usize,
    ) {
        self.mutate(|state| {
            state.counters.asr_queued_frame_count += 1;
            state.counters.asr_queued_pcm_bytes = state
                .counters
                .asr_queued_pcm_bytes
                .saturating_add(frame_bytes as u64);
            state.counters.asr_pending_frame_count += 1;
            state.counters.asr_pending_pcm_bytes = state
                .counters
                .asr_pending_pcm_bytes
                .saturating_add(frame_bytes as u64);
            for (segment_id, bytes) in shares {
                let bytes = *bytes as u64;
                let Some(segment_id) = *segment_id else {
                    state.counters.unknown_source_event_count =
                        state.counters.unknown_source_event_count.saturating_add(1);
                    state.counters.unknown_source_pcm_bytes = state
                        .counters
                        .unknown_source_pcm_bytes
                        .saturating_add(bytes);
                    continue;
                };
                if !ensure_segment_counter(state, segment_id, bytes) {
                    continue;
                }
                let counters = state
                    .segment_counters
                    .get_mut(&segment_id)
                    .expect("segment inserted");
                if shares.len() == 1 && bytes == frame_bytes as u64 {
                    counters.asr_queued_frame_count += 1;
                }
                counters.asr_queued_pcm_bytes = counters.asr_queued_pcm_bytes.saturating_add(bytes);
                counters.asr_pending_pcm_bytes =
                    counters.asr_pending_pcm_bytes.saturating_add(bytes);
                if shares.len() == 1 && bytes == frame_bytes as u64 {
                    counters.asr_pending_frame_count += 1;
                }
            }
        });
    }

    pub(crate) fn record_asr_send_completed_for_segment(
        &self,
        segment_id: Option<u32>,
        bytes: usize,
    ) {
        self.record_asr_send_completed_for_sources(&[(segment_id, bytes)], bytes);
    }

    pub(crate) fn record_asr_send_completed_for_sources(
        &self,
        shares: &[(Option<u32>, usize)],
        frame_bytes: usize,
    ) {
        self.mutate(|state| {
            state.counters.asr_send_completed_frame_count += 1;
            state.counters.asr_send_completed_pcm_bytes = state
                .counters
                .asr_send_completed_pcm_bytes
                .saturating_add(frame_bytes as u64);
            state.counters.asr_pending_frame_count =
                state.counters.asr_pending_frame_count.saturating_sub(1);
            state.counters.asr_pending_pcm_bytes = state
                .counters
                .asr_pending_pcm_bytes
                .saturating_sub(frame_bytes as u64);
            for (segment_id, bytes) in shares {
                let bytes = *bytes as u64;
                let Some(segment_id) = *segment_id else {
                    state.counters.unknown_source_event_count =
                        state.counters.unknown_source_event_count.saturating_add(1);
                    state.counters.unknown_source_pcm_bytes = state
                        .counters
                        .unknown_source_pcm_bytes
                        .saturating_add(bytes);
                    continue;
                };
                if !ensure_segment_counter(state, segment_id, bytes) {
                    continue;
                }
                let counters = state
                    .segment_counters
                    .get_mut(&segment_id)
                    .expect("segment inserted");
                if shares.len() == 1 && bytes == frame_bytes as u64 {
                    counters.asr_send_completed_frame_count += 1;
                    counters.asr_pending_frame_count =
                        counters.asr_pending_frame_count.saturating_sub(1);
                }
                counters.asr_send_completed_pcm_bytes =
                    counters.asr_send_completed_pcm_bytes.saturating_add(bytes);
                counters.asr_pending_pcm_bytes =
                    counters.asr_pending_pcm_bytes.saturating_sub(bytes);
            }
        });
        let mut settled = false;
        for segment_id in shares.iter().filter_map(|(segment_id, _)| *segment_id) {
            settled |= self.claim_final_delivery_snapshot(segment_id);
        }
        if settled {
            self.snapshot("asr_delivery_settled", true);
        }
    }

    pub(crate) fn record_asr_send_failed_for_segment(&self, segment_id: Option<u32>, bytes: usize) {
        self.record_asr_send_failed_for_sources(&[(segment_id, bytes)], bytes);
    }

    pub(crate) fn record_asr_send_failed_for_sources(
        &self,
        shares: &[(Option<u32>, usize)],
        frame_bytes: usize,
    ) {
        self.mutate(|state| {
            state.counters.asr_send_failed_frame_count += 1;
            state.counters.asr_send_failed_pcm_bytes = state
                .counters
                .asr_send_failed_pcm_bytes
                .saturating_add(frame_bytes as u64);
            state.counters.asr_pending_frame_count =
                state.counters.asr_pending_frame_count.saturating_sub(1);
            state.counters.asr_pending_pcm_bytes = state
                .counters
                .asr_pending_pcm_bytes
                .saturating_sub(frame_bytes as u64);
            for (segment_id, bytes) in shares {
                let bytes = *bytes as u64;
                let Some(segment_id) = *segment_id else {
                    state.counters.unknown_source_event_count =
                        state.counters.unknown_source_event_count.saturating_add(1);
                    state.counters.unknown_source_pcm_bytes = state
                        .counters
                        .unknown_source_pcm_bytes
                        .saturating_add(bytes);
                    continue;
                };
                if !ensure_segment_counter(state, segment_id, bytes) {
                    continue;
                }
                let counters = state
                    .segment_counters
                    .get_mut(&segment_id)
                    .expect("segment inserted");
                if shares.len() == 1 && bytes == frame_bytes as u64 {
                    counters.asr_send_failed_frame_count += 1;
                    counters.asr_pending_frame_count =
                        counters.asr_pending_frame_count.saturating_sub(1);
                }
                counters.asr_send_failed_pcm_bytes =
                    counters.asr_send_failed_pcm_bytes.saturating_add(bytes);
                counters.asr_pending_pcm_bytes =
                    counters.asr_pending_pcm_bytes.saturating_sub(bytes);
            }
        });
        self.snapshot("asr_send_failed", true);
    }

    fn claim_final_delivery_snapshot(&self, segment_id: u32) -> bool {
        let mut state = self.state.lock();
        let ready = state
            .segment_counters
            .get(&segment_id)
            .is_some_and(|counters| {
                segment_delivery_settled(counters)
                    && (state.capture_ended || segment_counters_settled(counters))
            });
        ready && state.segment_final_snapshot_emitted.insert(segment_id)
    }

    pub(crate) fn record_asr_queue_rejected(&self, bytes: usize) {
        self.mutate_active_counters(|c| {
            c.asr_queue_rejected_frame_count += 1;
            c.asr_queue_rejected_pcm_bytes =
                c.asr_queue_rejected_pcm_bytes.saturating_add(bytes as u64);
        });
        self.snapshot("asr_queue_rejected", true);
    }

    fn mutate_asr_queue_rejected_for_sources(
        &self,
        shares: &[(Option<u32>, usize)],
        frame_bytes: usize,
        settle_pending: bool,
    ) {
        self.mutate(|state| {
            state.counters.asr_queue_rejected_frame_count += 1;
            state.counters.asr_queue_rejected_pcm_bytes = state
                .counters
                .asr_queue_rejected_pcm_bytes
                .saturating_add(frame_bytes as u64);
            if settle_pending {
                state.counters.asr_pending_frame_count =
                    state.counters.asr_pending_frame_count.saturating_sub(1);
                state.counters.asr_pending_pcm_bytes = state
                    .counters
                    .asr_pending_pcm_bytes
                    .saturating_sub(frame_bytes as u64);
            }
            for (segment_id, bytes) in shares {
                let bytes = *bytes as u64;
                let Some(segment_id) = *segment_id else {
                    state.counters.unknown_source_event_count =
                        state.counters.unknown_source_event_count.saturating_add(1);
                    state.counters.unknown_source_pcm_bytes = state
                        .counters
                        .unknown_source_pcm_bytes
                        .saturating_add(bytes);
                    continue;
                };
                if !ensure_segment_counter(state, segment_id, bytes) {
                    continue;
                }
                let counters = state
                    .segment_counters
                    .get_mut(&segment_id)
                    .expect("segment inserted");
                if shares.len() == 1 && bytes == frame_bytes as u64 {
                    counters.asr_queue_rejected_frame_count += 1;
                    if settle_pending {
                        counters.asr_pending_frame_count =
                            counters.asr_pending_frame_count.saturating_sub(1);
                    }
                }
                counters.asr_queue_rejected_pcm_bytes =
                    counters.asr_queue_rejected_pcm_bytes.saturating_add(bytes);
                if settle_pending {
                    counters.asr_pending_pcm_bytes =
                        counters.asr_pending_pcm_bytes.saturating_sub(bytes);
                }
            }
        });
    }

    pub(crate) fn record_asr_queue_rejected_for_sources(
        &self,
        shares: &[(Option<u32>, usize)],
        frame_bytes: usize,
    ) {
        self.mutate_asr_queue_rejected_for_sources(shares, frame_bytes, true);
        self.snapshot("asr_queue_rejected", true);
    }

    pub(crate) fn record_asr_queue_rejected_without_queue_for_sources(
        &self,
        shares: &[(Option<u32>, usize)],
        frame_bytes: usize,
    ) {
        self.mutate_asr_queue_rejected_for_sources(shares, frame_bytes, false);
        self.snapshot("asr_queue_rejected", true);
    }

    pub(crate) fn record_asr_abandoned_for_sources(
        &self,
        shares: &[(Option<u32>, usize)],
        frame_bytes: usize,
    ) {
        self.mutate(|state| {
            state.counters.asr_abandoned_frame_count += 1;
            state.counters.asr_abandoned_pcm_bytes = state
                .counters
                .asr_abandoned_pcm_bytes
                .saturating_add(frame_bytes as u64);
            state.counters.asr_pending_frame_count =
                state.counters.asr_pending_frame_count.saturating_sub(1);
            state.counters.asr_pending_pcm_bytes = state
                .counters
                .asr_pending_pcm_bytes
                .saturating_sub(frame_bytes as u64);
            for (segment_id, bytes) in shares {
                let bytes = *bytes as u64;
                let Some(segment_id) = *segment_id else {
                    state.counters.unknown_source_event_count =
                        state.counters.unknown_source_event_count.saturating_add(1);
                    state.counters.unknown_source_pcm_bytes = state
                        .counters
                        .unknown_source_pcm_bytes
                        .saturating_add(bytes);
                    continue;
                };
                if !ensure_segment_counter(state, segment_id, bytes) {
                    continue;
                }
                let counters = state
                    .segment_counters
                    .get_mut(&segment_id)
                    .expect("segment inserted");
                if shares.len() == 1 && bytes == frame_bytes as u64 {
                    counters.asr_abandoned_frame_count += 1;
                    counters.asr_pending_frame_count =
                        counters.asr_pending_frame_count.saturating_sub(1);
                }
                counters.asr_abandoned_pcm_bytes =
                    counters.asr_abandoned_pcm_bytes.saturating_add(bytes);
                counters.asr_pending_pcm_bytes =
                    counters.asr_pending_pcm_bytes.saturating_sub(bytes);
            }
        });
        self.snapshot("asr_abandoned", true);
    }

    pub(crate) fn record_asr_unresolved_for_sources(
        &self,
        shares: &[(Option<u32>, usize)],
        bytes: usize,
    ) {
        self.mutate(|state| {
            state.counters.asr_unresolved_chunk_count += 1;
            state.counters.asr_unresolved_pcm_bytes = state
                .counters
                .asr_unresolved_pcm_bytes
                .saturating_add(bytes as u64);
            if shares.is_empty() {
                state.counters.unknown_source_event_count =
                    state.counters.unknown_source_event_count.saturating_add(1);
                state.counters.unknown_source_pcm_bytes = state
                    .counters
                    .unknown_source_pcm_bytes
                    .saturating_add(bytes as u64);
            }
            for (segment_id, share_bytes) in shares {
                let share_bytes = *share_bytes as u64;
                let Some(segment_id) = *segment_id else {
                    state.counters.unknown_source_event_count =
                        state.counters.unknown_source_event_count.saturating_add(1);
                    state.counters.unknown_source_pcm_bytes = state
                        .counters
                        .unknown_source_pcm_bytes
                        .saturating_add(share_bytes);
                    continue;
                };
                if !ensure_segment_counter(state, segment_id, share_bytes) {
                    continue;
                }
                let counters = state
                    .segment_counters
                    .get_mut(&segment_id)
                    .expect("segment inserted");
                counters.asr_unresolved_chunk_count += 1;
                counters.asr_unresolved_pcm_bytes = counters
                    .asr_unresolved_pcm_bytes
                    .saturating_add(share_bytes);
            }
        });
        self.snapshot("asr_unresolved", true);
    }

    pub(crate) fn snapshot(&self, reason: &'static str, force: bool) {
        let now = Instant::now();
        // Poll snapshots run on the live audio path. Serializing every bounded
        // evidence ledger once per second grows to tens of KB per poll; retain
        // the full ledger for terminal/error snapshots and test sinks instead.
        if !force && self.snapshot_sink.lock().is_none() {
            let summary = {
                let mut state = self.state.lock();
                if state
                    .last_snapshot_at
                    .is_some_and(|last| now.duration_since(last) < Duration::from_secs(1))
                {
                    return;
                }
                state.last_snapshot_at = Some(now);
                state.snapshot_seq = state.snapshot_seq.saturating_add(1);
                PipelineSummarySnapshot {
                    event: "embedded_audio_pipeline_snapshot",
                    detail: "summary",
                    reason,
                    capture_generation: self.capture_generation,
                    coordinator_session_id: state.coordinator_session_id.clone(),
                    embedded_session_id: state.embedded_session_id,
                    snapshot_seq: state.snapshot_seq,
                    monotonic_ms: now.duration_since(state.started_at).as_millis() as u64,
                    counters: state.counters,
                    segment_counters: state.segment_counters.clone(),
                    segment_ledger_incomplete: state.segment_ledger_incomplete,
                    interval_ledger_incomplete: state.interval_ledger_incomplete,
                    asr_destination_facts_incomplete: state.asr_destination_facts_incomplete,
                    pcm_stage_facts_incomplete: state.pcm_stage_ledger.capacity_drops().4,
                }
            };
            match serde_json::to_string(&summary) {
                Ok(payload) => log::info!("[obs-audio-pipeline] {payload}"),
                Err(_) => log::warn!("[obs-audio-pipeline] summary serialization failed"),
            }
            return;
        }
        let snapshot = {
            let mut state = self.state.lock();
            if !force
                && state
                    .last_snapshot_at
                    .is_some_and(|last| now.duration_since(last) < Duration::from_secs(1))
            {
                return;
            }
            state.last_snapshot_at = Some(now);
            state.snapshot_seq = state.snapshot_seq.saturating_add(1);
            let settled_segments = state
                .segment_counters
                .iter()
                .filter_map(|(segment_id, counters)| {
                    segment_counters_settled(counters).then_some(*segment_id)
                })
                .collect::<Vec<_>>();
            state
                .segment_final_snapshot_emitted
                .extend(settled_segments);
            PipelineSnapshot {
                event: "embedded_audio_pipeline_snapshot",
                reason,
                capture_generation: self.capture_generation,
                coordinator_session_id: state.coordinator_session_id.clone(),
                embedded_session_id: state.embedded_session_id,
                snapshot_seq: state.snapshot_seq,
                monotonic_ms: now.duration_since(state.started_at).as_millis() as u64,
                counters: state.counters,
                segment_counters: state.segment_counters.clone(),
                segment_ledger_overflow_count: state.segment_ledger_overflow_count,
                segment_ledger_overflow_bytes: state.segment_ledger_overflow_bytes,
                segment_ledger_incomplete: state.segment_ledger_incomplete,
                interval_ledger_overflow_count: state.interval_ledger_overflow_count,
                interval_ledger_overflow_bytes: state.interval_ledger_overflow_bytes,
                interval_ledger_incomplete: state.interval_ledger_incomplete,
                asr_destination_facts: state.asr_destination_facts.iter().cloned().collect(),
                asr_destination_fact_update_drop_count: state
                    .asr_destination_fact_update_drop_count,
                asr_destination_fact_update_drop_bytes: state
                    .asr_destination_fact_update_drop_bytes,
                asr_destination_share_drop_count: state.asr_destination_share_drop_count,
                asr_destination_share_drop_bytes: state.asr_destination_share_drop_bytes,
                asr_destination_fact_missing_metadata_count: state
                    .asr_destination_fact_missing_metadata_count,
                asr_destination_fact_missing_metadata_bytes: state
                    .asr_destination_fact_missing_metadata_bytes,
                asr_destination_fact_conflict_count: state.asr_destination_fact_conflict_count,
                asr_destination_fact_conflict_bytes: state.asr_destination_fact_conflict_bytes,
                asr_destination_facts_incomplete: state.asr_destination_facts_incomplete,
                pcm_stage_facts: state.pcm_stage_ledger.facts(),
                pcm_stage_fact_update_drop_count: state.pcm_stage_ledger.capacity_drops().0,
                pcm_stage_fact_update_drop_bytes: state.pcm_stage_ledger.capacity_drops().1,
                pcm_stage_share_drop_count: state.pcm_stage_ledger.capacity_drops().2,
                pcm_stage_share_drop_bytes: state.pcm_stage_ledger.capacity_drops().3,
                pcm_stage_facts_incomplete: state.pcm_stage_ledger.capacity_drops().4,
                collector_pcm_facts: state.collector_pcm_facts.iter().copied().collect(),
                collector_pcm_fact_drop_count: state.collector_pcm_fact_drop_count,
                collector_pcm_fact_drop_bytes: state.collector_pcm_fact_drop_bytes,
                collector_pcm_facts_incomplete: state.collector_pcm_facts_incomplete,
                candidate_facts: state.candidate_ledger.facts(),
                candidate_fact_drop_count: state.candidate_ledger.capacity_drops().0,
                candidate_fact_drop_bytes: state.candidate_ledger.capacity_drops().1,
                candidate_source_run_drop_count: state.candidate_ledger.capacity_drops().2,
                candidate_source_run_drop_bytes: state.candidate_ledger.capacity_drops().3,
                candidate_facts_incomplete: state.candidate_ledger.capacity_drops().4,
            }
        };
        let Ok(payload) = serde_json::to_string(&snapshot) else {
            log::warn!("[obs-audio-pipeline] snapshot serialization failed");
            return;
        };
        if let Some(sink) = self.snapshot_sink.lock().as_ref().cloned() {
            sink(payload.clone());
        }
        log::info!("[obs-audio-pipeline] {payload}");
    }

    #[cfg(test)]
    fn counters_for_test(&self) -> PipelineCounters {
        self.state.lock().counters
    }

    #[cfg(test)]
    fn segment_counters_for_test(&self, segment_id: u32) -> PipelineCounters {
        self.state
            .lock()
            .segment_counters
            .get(&segment_id)
            .copied()
            .unwrap_or_default()
    }

    #[cfg(test)]
    pub(crate) fn asr_delivery_counts_for_test(&self, segment_id: u32) -> (u64, u64, u64, u64) {
        let counters = self.segment_counters_for_test(segment_id);
        (
            counters.asr_queued_pcm_bytes,
            counters.asr_send_completed_pcm_bytes,
            counters.asr_send_failed_pcm_bytes,
            counters.asr_pending_pcm_bytes,
        )
    }

    #[cfg(test)]
    pub(crate) fn asr_terminal_counts_for_test(
        &self,
        segment_id: u32,
    ) -> (u64, u64, u64, u64, u64, u64) {
        let counters = self.segment_counters_for_test(segment_id);
        (
            counters.asr_send_completed_pcm_bytes,
            counters.asr_send_failed_pcm_bytes,
            counters.asr_abandoned_pcm_bytes,
            counters.asr_unresolved_pcm_bytes,
            counters.asr_pending_pcm_bytes,
            counters.asr_queue_rejected_pcm_bytes,
        )
    }

    #[cfg(test)]
    pub(crate) fn asr_destination_facts_for_test(&self) -> Vec<AsrDestinationFact> {
        self.state
            .lock()
            .asr_destination_facts
            .iter()
            .cloned()
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn asr_destination_capacity_drops_for_test(&self) -> (u64, u64, u64, u64, bool) {
        let state = self.state.lock();
        (
            state.asr_destination_fact_update_drop_count,
            state.asr_destination_fact_update_drop_bytes,
            state.asr_destination_share_drop_count,
            state.asr_destination_share_drop_bytes,
            state.asr_destination_facts_incomplete,
        )
    }

    #[cfg(test)]
    pub(crate) fn asr_destination_fact_missing_metadata_for_test(&self) -> (u64, u64) {
        let state = self.state.lock();
        (
            state.asr_destination_fact_missing_metadata_count,
            state.asr_destination_fact_missing_metadata_bytes,
        )
    }

    #[cfg(test)]
    pub(crate) fn asr_destination_fact_conflicts_for_test(&self) -> (u64, u64) {
        let state = self.state.lock();
        (
            state.asr_destination_fact_conflict_count,
            state.asr_destination_fact_conflict_bytes,
        )
    }

    #[cfg(test)]
    pub(crate) fn pcm_stage_facts_for_test(&self) -> Vec<PcmStageMappingFact> {
        self.state.lock().pcm_stage_ledger.facts()
    }

    #[cfg(test)]
    pub(crate) fn pcm_stage_capacity_drops_for_test(&self) -> (u64, u64, u64, u64, bool) {
        let state = self.state.lock();
        state.pcm_stage_ledger.capacity_drops()
    }

    #[cfg(test)]
    pub(crate) fn candidate_facts_for_test(&self) -> Vec<CandidateFact> {
        self.state.lock().candidate_ledger.facts()
    }

    #[cfg(test)]
    pub(crate) fn collector_pcm_facts_for_test(&self) -> Vec<CollectorPcmChunkFact> {
        self.state
            .lock()
            .collector_pcm_facts
            .iter()
            .copied()
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn collector_pcm_fact_capacity_drops_for_test(&self) -> (u64, u64, bool) {
        let state = self.state.lock();
        (
            state.collector_pcm_fact_drop_count,
            state.collector_pcm_fact_drop_bytes,
            state.collector_pcm_facts_incomplete,
        )
    }
}

#[derive(Serialize)]
struct PipelineSummarySnapshot {
    event: &'static str,
    detail: &'static str,
    reason: &'static str,
    capture_generation: u64,
    coordinator_session_id: Option<String>,
    embedded_session_id: Option<u32>,
    snapshot_seq: u64,
    monotonic_ms: u64,
    counters: PipelineCounters,
    segment_counters: BTreeMap<u32, PipelineCounters>,
    segment_ledger_incomplete: bool,
    interval_ledger_incomplete: bool,
    asr_destination_facts_incomplete: bool,
    pcm_stage_facts_incomplete: bool,
}

#[derive(Serialize)]
struct PipelineSnapshot {
    event: &'static str,
    reason: &'static str,
    capture_generation: u64,
    coordinator_session_id: Option<String>,
    embedded_session_id: Option<u32>,
    snapshot_seq: u64,
    monotonic_ms: u64,
    counters: PipelineCounters,
    segment_counters: BTreeMap<u32, PipelineCounters>,
    segment_ledger_overflow_count: u64,
    segment_ledger_overflow_bytes: u64,
    segment_ledger_incomplete: bool,
    interval_ledger_overflow_count: u64,
    interval_ledger_overflow_bytes: u64,
    interval_ledger_incomplete: bool,
    asr_destination_facts: Vec<AsrDestinationFact>,
    asr_destination_fact_update_drop_count: u64,
    asr_destination_fact_update_drop_bytes: u64,
    asr_destination_share_drop_count: u64,
    asr_destination_share_drop_bytes: u64,
    asr_destination_fact_missing_metadata_count: u64,
    asr_destination_fact_missing_metadata_bytes: u64,
    asr_destination_fact_conflict_count: u64,
    asr_destination_fact_conflict_bytes: u64,
    asr_destination_facts_incomplete: bool,
    pcm_stage_facts: Vec<PcmStageMappingFact>,
    pcm_stage_fact_update_drop_count: u64,
    pcm_stage_fact_update_drop_bytes: u64,
    pcm_stage_share_drop_count: u64,
    pcm_stage_share_drop_bytes: u64,
    pcm_stage_facts_incomplete: bool,
    collector_pcm_facts: Vec<CollectorPcmChunkFact>,
    collector_pcm_fact_drop_count: u64,
    collector_pcm_fact_drop_bytes: u64,
    collector_pcm_facts_incomplete: bool,
    candidate_facts: Vec<CandidateFact>,
    candidate_fact_drop_count: u64,
    candidate_fact_drop_bytes: u64,
    candidate_source_run_drop_count: u64,
    candidate_source_run_drop_bytes: u64,
    candidate_facts_incomplete: bool,
}

pub(crate) struct EmbeddedAudioPipelineCaptureGuard {
    capture_generation: u64,
    observation: Arc<EmbeddedAudioPipelineObservation>,
}

impl EmbeddedAudioPipelineCaptureGuard {
    pub(crate) fn observation(&self) -> Arc<EmbeddedAudioPipelineObservation> {
        Arc::clone(&self.observation)
    }
}

impl Drop for EmbeddedAudioPipelineCaptureGuard {
    fn drop(&mut self) {
        self.observation.state.lock().capture_ended = true;
        self.observation.snapshot("capture_end", true);
        if let Some(registry) = PIPELINE_OBSERVATIONS.get() {
            let mut registry = registry.lock();
            if registry
                .get(&self.capture_generation)
                .is_some_and(|current| Arc::ptr_eq(current, &self.observation))
            {
                registry.remove(&self.capture_generation);
            }
        }
    }
}

static PIPELINE_OBSERVATIONS: OnceLock<Mutex<HashMap<u64, Arc<EmbeddedAudioPipelineObservation>>>> =
    OnceLock::new();

pub(crate) fn begin_embedded_audio_pipeline_capture(
    capture_generation: u64,
) -> EmbeddedAudioPipelineCaptureGuard {
    let observation = Arc::new(EmbeddedAudioPipelineObservation::new(capture_generation));
    observation.snapshot("capture_start", true);
    PIPELINE_OBSERVATIONS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .insert(capture_generation, Arc::clone(&observation));
    EmbeddedAudioPipelineCaptureGuard {
        capture_generation,
        observation,
    }
}

pub(crate) fn pipeline_observation(
    capture_generation: u64,
) -> Option<Arc<EmbeddedAudioPipelineObservation>> {
    PIPELINE_OBSERVATIONS
        .get()
        .and_then(|registry| registry.lock().get(&capture_generation).cloned())
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
    ProviderStream,
    FinalSupplement,
}

impl PreviewSource {
    fn first_event_name(self) -> &'static str {
        match self {
            Self::ProviderStream => "embedded_audio_preview_first_provider_stream",
            Self::FinalSupplement => "embedded_audio_preview_first_final_supplement",
        }
    }

    fn update_event_name(self) -> &'static str {
        match self {
            Self::ProviderStream => "embedded_audio_preview_provider_stream",
            Self::FinalSupplement => "embedded_audio_preview_final_supplement",
        }
    }
}

pub(crate) fn record_embedded_audio_preview_published(
    session_id: SessionId,
    preview_source: PreviewSource,
    after_stop: bool,
) {
    let now = Instant::now();
    let (event, event_name) = {
        let mut observations = audio_observations().lock();
        let Some(observation) = observations.get_mut(&session_id) else {
            return;
        };
        let first_preview = !observation.first_preview_observed;
        let elapsed = if first_preview {
            observation.first_preview_observed = true;
            elapsed_ms(observation.started_at, now)
        } else {
            0
        };
        let lifecycle = if after_stop {
            BleLifecycleState::ConnectedIdle
        } else {
            BleLifecycleState::Recording
        };
        (
            observation.next_event(
                now,
                EventSource::Provider,
                Capability::Audio,
                lifecycle,
                CommandResult::Started,
                ErrorCategory::None,
                if first_preview {
                    TimingMetric::PreviewLatencyMs
                } else {
                    TimingMetric::None
                },
                elapsed,
            ),
            if first_preview {
                preview_source.first_event_name()
            } else {
                preview_source.update_event_name()
            },
        )
    };
    emit(event_name, event);
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
        // Category-only obs events drop the diagnostic string; always log it too.
        log::error!(
            "[obs-v1] ota_gatt_transfer_failed category={error:?} elapsed_ms={transfer_elapsed_ms} detail={message}"
        );
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
        log::error!("[obs-v1] ota_control_handoff_failed category={error:?} detail={message}");
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
    fn candidate_facts_keep_release_attempt_and_accept_as_one_operation() {
        let observation = EmbeddedAudioPipelineObservation::new(41);
        let candidate_range = CandidateRange {
            start: 320,
            end: 640,
        };
        let destination_range = PcmRange { start: 0, end: 320 };
        let source_run = CandidateSourceRunFact {
            capture_generation: Some(41),
            segment_id: Some(7),
            candidate_range: Some(candidate_range),
            collector_metadata: None,
            collector_emitted_range: None,
            bytes: 320,
        };
        let common = |kind, destination_range| CandidateFact {
            candidate_id: 9,
            operation_id: Some(3),
            kind,
            candidate_range: Some(candidate_range),
            kws_feed_range: None,
            release_destination_range: destination_range,
            release_coordinator_session_id: Some("session-1".to_string()),
            release_source_stream_id: Some(99),
            source_runs: vec![source_run],
            bytes: 320,
            reason: None,
        };
        observation.record_candidate_fact(common(CandidateFactKind::ReleaseAttempted, None));
        observation.record_candidate_fact(common(
            CandidateFactKind::ReleaseAccepted,
            Some(destination_range),
        ));

        let facts = observation.candidate_facts_for_test();
        assert_eq!(facts.len(), 2);
        assert_eq!(facts[0].candidate_id, facts[1].candidate_id);
        assert_eq!(facts[0].operation_id, facts[1].operation_id);
        assert_eq!(facts[0].source_runs[0].capture_generation, Some(41));
        assert_eq!(facts[0].source_runs[0].segment_id, Some(7));
        assert_eq!(facts[1].release_destination_range, Some(destination_range));
    }

    #[test]
    fn candidate_fact_capacity_is_bounded_and_marks_incomplete() {
        let mut ledger = CandidateFactLedger::default();
        for operation_id in 1..=(CANDIDATE_FACT_CAPACITY as u64 + 1) {
            ledger.record(CandidateFact {
                candidate_id: 12,
                operation_id: Some(operation_id),
                kind: CandidateFactKind::Buffered,
                candidate_range: Some(CandidateRange {
                    start: operation_id,
                    end: operation_id + 1,
                }),
                kws_feed_range: None,
                release_destination_range: None,
                release_coordinator_session_id: None,
                release_source_stream_id: None,
                source_runs: Vec::new(),
                bytes: 1,
                reason: None,
            });
        }

        let (drop_count, drop_bytes, source_drop_count, source_drop_bytes, incomplete) =
            ledger.capacity_drops();
        assert_eq!(ledger.facts().len(), CANDIDATE_FACT_CAPACITY);
        assert_eq!(drop_count, 1);
        assert_eq!(drop_bytes, 1);
        assert_eq!(source_drop_count, 0);
        assert_eq!(source_drop_bytes, 0);
        assert!(incomplete);
    }

    #[test]
    fn candidate_unknown_source_run_is_retained_as_unknown_not_dropped() {
        let mut ledger = CandidateFactLedger::default();
        ledger.record(CandidateFact {
            candidate_id: 13,
            operation_id: Some(1),
            kind: CandidateFactKind::ReleaseAttempted,
            candidate_range: Some(CandidateRange { start: 0, end: 8 }),
            kws_feed_range: None,
            release_destination_range: None,
            release_coordinator_session_id: Some("session-2".to_string()),
            release_source_stream_id: Some(100),
            source_runs: vec![CandidateSourceRunFact {
                capture_generation: None,
                segment_id: None,
                candidate_range: None,
                collector_metadata: None,
                collector_emitted_range: None,
                bytes: 0,
            }],
            bytes: 8,
            reason: Some("source_coordinates_unknown".to_string()),
        });

        let facts = ledger.facts();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].source_runs.len(), 1);
        assert!(facts[0].source_runs[0].candidate_range.is_none());
        assert!(ledger.capacity_drops().4);
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
