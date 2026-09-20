//! Session-scoped source integrity for automatic delivery.
//!
//! This module deliberately does not participate in the dictation FSM or in
//! transport admission. It records only a confirmed collector revision
//! conflict and exposes one qualification function for the later production
//! boundaries. Diagnostic incompleteness remains separate from a product
//! conflict.

use std::sync::Arc;

use parking_lot::Mutex;

use crate::coordinator_state::SessionId;
use crate::embedded_audio::{StreamingPcmChunkMetadata, StreamingPcmRange};

use super::dictation::{
    CaptureAdmissionBindingStatus, SourceAdmissionDependency, SourceAdmissionDependencyLedger,
    SourceAdmissionOperationOwner, SourceAdmissionUse,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SourceIntegrityOwner {
    Candidate(u64),
    Session(SessionId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RevisionConflictEvidence {
    pub(super) collector_instance_id: Option<u64>,
    pub(super) segment_ordinal: u64,
    pub(super) packet_sequence: u16,
    pub(super) packet_revision: u32,
    pub(super) emission_ordinal: u64,
    pub(super) emitted_range: StreamingPcmRange,
}

impl RevisionConflictEvidence {
    fn from_metadata(metadata: StreamingPcmChunkMetadata) -> Self {
        Self {
            collector_instance_id: metadata.collector_instance_id,
            segment_ordinal: metadata.segment_ordinal,
            packet_sequence: metadata.packet_sequence,
            packet_revision: metadata.packet_revision,
            emission_ordinal: metadata.emission_ordinal,
            emitted_range: metadata.emitted_range,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceIntegrityState {
    NoKnownConflict,
    RevisionConflict {
        first: RevisionConflictEvidence,
        count: u64,
    },
}

#[derive(Debug)]
struct SourceIntegrityInner {
    owner: SourceIntegrityOwner,
    state: SourceIntegrityState,
}

/// Shared handle owned by one candidate or logical dictation session.
///
/// Cloning the handle shares the same monotonic state. Promotion must rebind
/// this handle rather than copying a boolean, so an event racing a candidate
/// promotion cannot update a stale snapshot.
#[derive(Debug, Clone)]
pub(super) struct SourceIntegrityHandle {
    inner: Arc<Mutex<SourceIntegrityInner>>,
}

impl SourceIntegrityHandle {
    pub(super) fn for_candidate(candidate_id: u64) -> Self {
        Self {
            inner: Arc::new(Mutex::new(SourceIntegrityInner {
                owner: SourceIntegrityOwner::Candidate(candidate_id),
                state: SourceIntegrityState::NoKnownConflict,
            })),
        }
    }

    pub(super) fn owner(&self) -> SourceIntegrityOwner {
        self.inner.lock().owner
    }

    /// Rebind one handle across candidate promotion or logical continuation.
    /// A stale owner cannot steal the handle from the current session.
    pub(super) fn rebind_owner(
        &self,
        expected_owner: SourceIntegrityOwner,
        next_owner: SourceIntegrityOwner,
    ) -> bool {
        let mut inner = self.inner.lock();
        if inner.owner != expected_owner {
            return false;
        }
        inner.owner = next_owner;
        true
    }

    /// Record only a confirmed revision conflict. Missing metadata, overflow,
    /// and unknown coordinates intentionally do not change this state.
    pub(super) fn observe_metadata(&self, metadata: StreamingPcmChunkMetadata) {
        if !metadata.revision_conflict {
            return;
        }
        let evidence = RevisionConflictEvidence::from_metadata(metadata);
        let mut inner = self.inner.lock();
        inner.state = match inner.state {
            SourceIntegrityState::NoKnownConflict => SourceIntegrityState::RevisionConflict {
                first: evidence,
                count: 1,
            },
            SourceIntegrityState::RevisionConflict { first, count } => {
                SourceIntegrityState::RevisionConflict {
                    first,
                    count: count.saturating_add(1),
                }
            }
        };
    }

    pub(super) fn status(&self) -> SourceIntegrityStatus {
        match self.inner.lock().state {
            SourceIntegrityState::NoKnownConflict => SourceIntegrityStatus::NoKnownConflict,
            SourceIntegrityState::RevisionConflict { .. } => {
                SourceIntegrityStatus::RevisionConflict
            }
        }
    }

    pub(super) fn first_conflict(&self) -> Option<RevisionConflictEvidence> {
        match self.inner.lock().state {
            SourceIntegrityState::NoKnownConflict => None,
            SourceIntegrityState::RevisionConflict { first, .. } => Some(first),
        }
    }

    pub(super) fn conflict_count(&self) -> u64 {
        match self.inner.lock().state {
            SourceIntegrityState::NoKnownConflict => 0,
            SourceIntegrityState::RevisionConflict { count, .. } => count,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SourceIntegrityStatus {
    NoKnownConflict,
    RevisionConflict,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum SourceIntegrityQualification {
    Allowed,
    SourceIntegrityBlocked,
    OwnerMismatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum SourceIntegrityEvidenceCompleteness {
    Complete,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum SourceIntegrityDecisionReason {
    OwnerMismatch,
    ActorRevisionConflict,
    SessionBodyInputSuperseded,
    SessionBodyInputPossibleSuperseded,
    CandidateGateInputSuperseded,
    IncompleteSourceDependency,
    NoConfirmedConflict,
}

/// The first traceable fact behind a unified decision. It deliberately holds
/// identity only; it does not copy PCM or reconstruct evidence from the
/// bounded diagnostic projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SourceIntegrityEvidence {
    pub(super) reason: SourceIntegrityDecisionReason,
    pub(super) capture_generation: Option<u64>,
    pub(super) collector_instance_id: Option<u64>,
    pub(super) reset_epoch: Option<u64>,
    pub(super) notification_id: Option<u64>,
    pub(super) segment_ordinal: Option<u64>,
    pub(super) packet_revision: Option<u32>,
    pub(super) emission_ordinal: Option<u64>,
    pub(super) emitted_range: Option<StreamingPcmRange>,
    pub(super) operation_owner: Option<SourceAdmissionOperationOwner>,
    pub(super) use_kind: Option<SourceAdmissionUse>,
    pub(super) operation_id: Option<u64>,
    pub(super) capture_admission_id: Option<u64>,
    pub(super) packet_sequence: Option<u16>,
    pub(super) superseded_by_admission_id: Option<u64>,
    pub(super) superseded_by_notification_id: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SourceIntegrityConfirmedConflict {
    pub(super) reason: SourceIntegrityDecisionReason,
    pub(super) evidence: SourceIntegrityEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SourceIntegrityDecision {
    pub(super) verdict: SourceIntegrityQualification,
    pub(super) evidence_completeness: SourceIntegrityEvidenceCompleteness,
    pub(super) reason: SourceIntegrityDecisionReason,
    pub(super) first_evidence: Option<SourceIntegrityEvidence>,
}

impl SourceIntegrityDecision {
    fn owner_mismatch() -> Self {
        Self {
            verdict: SourceIntegrityQualification::OwnerMismatch,
            evidence_completeness: SourceIntegrityEvidenceCompleteness::Unknown,
            reason: SourceIntegrityDecisionReason::OwnerMismatch,
            first_evidence: None,
        }
    }

    fn blocked(
        reason: SourceIntegrityDecisionReason,
        first_evidence: SourceIntegrityEvidence,
    ) -> Self {
        Self {
            verdict: SourceIntegrityQualification::SourceIntegrityBlocked,
            evidence_completeness: SourceIntegrityEvidenceCompleteness::Complete,
            reason,
            first_evidence: Some(first_evidence),
        }
    }

    fn allowed(
        evidence_completeness: SourceIntegrityEvidenceCompleteness,
        reason: SourceIntegrityDecisionReason,
        first_evidence: Option<SourceIntegrityEvidence>,
    ) -> Self {
        Self {
            verdict: SourceIntegrityQualification::Allowed,
            evidence_completeness,
            reason,
            first_evidence,
        }
    }
}

fn dependency_evidence(
    dependency: &SourceAdmissionDependency,
    reason: SourceIntegrityDecisionReason,
) -> SourceIntegrityEvidence {
    let operation = dependency.operations.front().copied();
    let witness = dependency
        .capture_receipt
        .as_ref()
        .map(|receipt| receipt.witness());
    let actor_metadata = dependency.actor_chunk_metadata;
    SourceIntegrityEvidence {
        reason,
        capture_generation: dependency.capture_generation,
        collector_instance_id: witness
            .as_ref()
            .and_then(|witness| witness.collector_instance_id)
            .or_else(|| {
                dependency
                    .capture_fact
                    .as_ref()
                    .and_then(|fact| fact.collector_instance_id)
            }),
        reset_epoch: witness
            .as_ref()
            .and_then(|witness| witness.reset_epoch)
            .or_else(|| {
                dependency
                    .capture_fact
                    .as_ref()
                    .and_then(|fact| fact.reset_epoch)
            }),
        notification_id: witness
            .as_ref()
            .and_then(|witness| witness.notification_id)
            .or_else(|| {
                dependency
                    .capture_fact
                    .as_ref()
                    .and_then(|fact| fact.notification_id)
            }),
        segment_ordinal: actor_metadata.map(|metadata| metadata.segment_ordinal),
        packet_revision: actor_metadata.map(|metadata| metadata.packet_revision),
        emission_ordinal: actor_metadata.map(|metadata| metadata.emission_ordinal),
        emitted_range: actor_metadata.map(|metadata| metadata.emitted_range),
        operation_owner: Some(dependency.operation_owner),
        use_kind: Some(dependency.use_kind),
        operation_id: operation.and_then(|operation| operation.operation_id),
        capture_admission_id: witness
            .as_ref()
            .map(|witness| witness.admission_id)
            .or_else(|| {
                dependency
                    .capture_fact
                    .as_ref()
                    .and_then(|fact| fact.admission_id)
            }),
        packet_sequence: witness
            .as_ref()
            .map(|witness| witness.packet_sequence)
            .or_else(|| {
                dependency
                    .capture_fact
                    .as_ref()
                    .and_then(|fact| fact.packet_sequence)
            }),
        superseded_by_admission_id: witness
            .as_ref()
            .and_then(|witness| witness.superseded_by_admission_id),
        superseded_by_notification_id: witness
            .as_ref()
            .and_then(|witness| witness.superseded_by_notification_id),
    }
}

fn actor_conflict_evidence(evidence: RevisionConflictEvidence) -> SourceIntegrityEvidence {
    SourceIntegrityEvidence {
        reason: SourceIntegrityDecisionReason::ActorRevisionConflict,
        capture_generation: None,
        collector_instance_id: evidence.collector_instance_id,
        reset_epoch: None,
        notification_id: None,
        segment_ordinal: Some(evidence.segment_ordinal),
        packet_revision: Some(evidence.packet_revision),
        emission_ordinal: Some(evidence.emission_ordinal),
        emitted_range: Some(evidence.emitted_range),
        operation_owner: None,
        use_kind: None,
        operation_id: None,
        capture_admission_id: None,
        packet_sequence: Some(evidence.packet_sequence),
        superseded_by_admission_id: None,
        superseded_by_notification_id: None,
    }
}

fn dependency_has_complete_operation(dependency: &SourceAdmissionDependency) -> bool {
    dependency.accepted_bytes > 0
        && dependency.operations.iter().all(|operation| {
            operation.operation_id.is_some()
                && operation.owner_range.is_some()
                && operation.source_interval.is_some()
        })
}

fn dependency_has_complete_observation(
    dependency: &SourceAdmissionDependency,
    witness: Option<&crate::embedded_audio::SessionAdmissionWitness>,
) -> bool {
    dependency.status_at_acceptance == CaptureAdmissionBindingStatus::MatchedConsumed
        && dependency.capture_receipt.is_some()
        && !dependency.non_projection_incomplete
        && dependency_has_complete_operation(dependency)
        && witness.is_some_and(|witness| !witness.metadata_incomplete)
}

fn dependency_is_definite_session_body(
    dependency: &SourceAdmissionDependency,
    witness: Option<&crate::embedded_audio::SessionAdmissionWitness>,
) -> bool {
    dependency.use_kind == SourceAdmissionUse::SessionBodyInput
        && dependency_has_complete_observation(dependency, witness)
}

fn latch_and_return_blocked(
    ledger: &mut SourceAdmissionDependencyLedger,
    reason: SourceIntegrityDecisionReason,
    evidence: SourceIntegrityEvidence,
) -> SourceIntegrityDecision {
    ledger.latch_source_integrity_conflict(SourceIntegrityConfirmedConflict { reason, evidence });
    let conflict = ledger
        .confirmed_source_integrity_conflict()
        .expect("source integrity conflict was latched");
    SourceIntegrityDecision::blocked(conflict.reason, conflict.evidence.clone())
}

/// Unified, read-only source-integrity qualification for a future production
/// boundary. The active dependency ledger is the security input; bounded
/// projections are intentionally not consulted.
pub(super) fn qualify_active_source_integrity(
    expected_owner: SourceIntegrityOwner,
    current_logical_owner: SourceIntegrityOwner,
    ledger: &mut SourceAdmissionDependencyLedger,
    actor_conflict: Option<RevisionConflictEvidence>,
) -> SourceIntegrityDecision {
    if expected_owner != current_logical_owner {
        return SourceIntegrityDecision::owner_mismatch();
    }

    if let Some(conflict) = ledger.confirmed_source_integrity_conflict() {
        return SourceIntegrityDecision::blocked(conflict.reason, conflict.evidence.clone());
    }

    if let Some(actor_conflict) = actor_conflict {
        return latch_and_return_blocked(
            ledger,
            SourceIntegrityDecisionReason::ActorRevisionConflict,
            actor_conflict_evidence(actor_conflict),
        );
    }

    let mut first_unknown = None;
    let mut confirmed_conflict = None;
    for dependency in ledger.active_dependencies() {
        // Take one witness snapshot per dependency. The decision and its
        // traceable evidence must describe the same observed revision.
        let witness = dependency
            .capture_receipt
            .as_ref()
            .map(|receipt| receipt.witness());
        let superseded = witness.as_ref().is_some_and(|witness| witness.superseded);
        let definite_session_body =
            dependency_is_definite_session_body(dependency, witness.as_ref());

        if definite_session_body && superseded {
            confirmed_conflict = Some((
                SourceIntegrityDecisionReason::SessionBodyInputSuperseded,
                dependency_evidence(
                    dependency,
                    SourceIntegrityDecisionReason::SessionBodyInputSuperseded,
                ),
            ));
            break;
        }

        if dependency.use_kind == SourceAdmissionUse::SessionBodyInput
            && dependency
                .actor_chunk_metadata
                .is_some_and(|metadata| metadata.revision_conflict)
            && definite_session_body
        {
            confirmed_conflict = Some((
                SourceIntegrityDecisionReason::ActorRevisionConflict,
                dependency_evidence(
                    dependency,
                    SourceIntegrityDecisionReason::ActorRevisionConflict,
                ),
            ));
            break;
        }

        if superseded && dependency.use_kind == SourceAdmissionUse::SessionBodyInputPossible {
            first_unknown.get_or_insert_with(|| {
                dependency_evidence(
                    dependency,
                    SourceIntegrityDecisionReason::SessionBodyInputPossibleSuperseded,
                )
            });
        } else if superseded && dependency.use_kind == SourceAdmissionUse::CandidateGateInput {
            first_unknown.get_or_insert_with(|| {
                dependency_evidence(
                    dependency,
                    SourceIntegrityDecisionReason::CandidateGateInputSuperseded,
                )
            });
        } else if !dependency_has_complete_observation(dependency, witness.as_ref())
            || dependency.tracking_incomplete
        {
            first_unknown.get_or_insert_with(|| {
                dependency_evidence(
                    dependency,
                    SourceIntegrityDecisionReason::IncompleteSourceDependency,
                )
            });
        }
    }

    if let Some((reason, evidence)) = confirmed_conflict {
        return latch_and_return_blocked(ledger, reason, evidence);
    }

    if let Some(evidence) = first_unknown {
        return SourceIntegrityDecision::allowed(
            SourceIntegrityEvidenceCompleteness::Unknown,
            evidence.reason,
            Some(evidence),
        );
    }

    if ledger.has_non_projection_incomplete() {
        return SourceIntegrityDecision::allowed(
            SourceIntegrityEvidenceCompleteness::Unknown,
            SourceIntegrityDecisionReason::IncompleteSourceDependency,
            None,
        );
    }

    SourceIntegrityDecision::allowed(
        SourceIntegrityEvidenceCompleteness::Complete,
        SourceIntegrityDecisionReason::NoConfirmedConflict,
        None,
    )
}

/// The one pure qualification entry for future production boundaries.
///
/// `OwnerMismatch` is intentionally distinct from `SourceIntegrityBlocked`:
/// a stale callback must not be allowed to qualify a new session, and a clean
/// session must not be blocked by a different session's handle.
pub(super) fn qualify_session_integrity(
    handle: &SourceIntegrityHandle,
    expected_owner: SourceIntegrityOwner,
) -> SourceIntegrityQualification {
    let inner = handle.inner.lock();
    if inner.owner != expected_owner {
        return SourceIntegrityQualification::OwnerMismatch;
    }
    match inner.state {
        SourceIntegrityState::NoKnownConflict => SourceIntegrityQualification::Allowed,
        SourceIntegrityState::RevisionConflict { .. } => {
            SourceIntegrityQualification::SourceIntegrityBlocked
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn admitted_audio(
        replaced: bool,
    ) -> (
        crate::embedded_audio::SessionAdmissionFact,
        crate::embedded_audio::SessionAdmissionReceipt,
    ) {
        let mut collector = crate::embedded_audio::SessionCollector::default();
        collector
            .handle_notification(&crate::embedded_audio::build_session_start_notification(7))
            .expect("session start");
        collector
            .handle_notification(
                &crate::embedded_audio::build_audio_data_notification(7, 0, &[1, 2])
                    .expect("initial audio"),
            )
            .expect("initial audio event");
        let fact = collector.last_admission_fact().expect("audio fact");
        let receipt = collector.last_admission_receipt().expect("audio receipt");
        if replaced {
            collector
                .handle_notification(
                    &crate::embedded_audio::build_audio_data_notification(7, 0, &[1, 2, 3, 4])
                        .expect("replacement audio"),
                )
                .expect("replacement audio event");
        }
        (fact, receipt)
    }

    fn admitted_audio_with_collector() -> (
        crate::embedded_audio::SessionAdmissionFact,
        crate::embedded_audio::SessionAdmissionReceipt,
        crate::embedded_audio::SessionCollector,
    ) {
        let mut collector = crate::embedded_audio::SessionCollector::default();
        collector
            .handle_notification(&crate::embedded_audio::build_session_start_notification(7))
            .expect("session start");
        collector
            .handle_notification(
                &crate::embedded_audio::build_audio_data_notification(7, 0, &[1, 2])
                    .expect("initial audio"),
            )
            .expect("initial audio event");
        let fact = collector.last_admission_fact().expect("audio fact");
        let receipt = collector.last_admission_receipt().expect("audio receipt");
        (fact, receipt, collector)
    }

    fn pcm_source_interval(start: u64, end: u64) -> crate::observability::PcmSourceInterval {
        crate::observability::PcmSourceInterval {
            capture_generation: 1,
            source_stream_id: 1,
            stream_kind: crate::observability::PcmStreamKind::CoordinatorInputPcm,
            mapping: crate::observability::PcmMappingKind::PositionPreserving,
            segment_id: Some(7),
            range: crate::observability::PcmRange { start, end },
        }
    }

    fn dependency(
        owner: SourceAdmissionOperationOwner,
        use_kind: SourceAdmissionUse,
        fact: crate::embedded_audio::SessionAdmissionFact,
        receipt: crate::embedded_audio::SessionAdmissionReceipt,
        operation_id: Option<u64>,
        owner_range: Option<(u64, u64)>,
        source_range: Option<(u64, u64)>,
    ) -> SourceAdmissionDependency {
        let operation_owner_range = owner_range.map(|(start, end)| match owner {
            SourceAdmissionOperationOwner::Candidate { .. } => {
                super::super::dictation::SourceAdmissionOwnerRange::Candidate { start, end }
            }
            SourceAdmissionOperationOwner::Session { .. } => {
                super::super::dictation::SourceAdmissionOwnerRange::Session { start, end }
            }
        });
        let source_interval = source_range.map(|(start, end)| pcm_source_interval(start, end));
        let packet_sequence = fact.packet_sequence.unwrap_or_default();
        SourceAdmissionDependency {
            operation_owner: owner,
            capture_generation: Some(1),
            capture_fact: Some(fact),
            capture_receipt: Some(receipt),
            actor_fact: None,
            actor_chunk_metadata: Some(metadata(false, packet_sequence)),
            use_kind,
            accepted_bytes: source_interval.map_or(0, |interval| interval.len()),
            operations: std::collections::VecDeque::from([
                super::super::dictation::SourceAdmissionOperation {
                    owner,
                    operation_id,
                    owner_range: operation_owner_range,
                    source_interval,
                    accepted_bytes: source_interval.map_or(0, |interval| interval.len()),
                },
            ]),
            status_at_acceptance: CaptureAdmissionBindingStatus::MatchedConsumed,
            tracking_incomplete: operation_id.is_none()
                || operation_owner_range.is_none()
                || source_interval.is_none()
                || use_kind == SourceAdmissionUse::SessionBodyInputPossible,
            non_projection_incomplete: operation_id.is_none()
                || operation_owner_range.is_none()
                || source_interval.is_none()
                || use_kind == SourceAdmissionUse::SessionBodyInputPossible,
        }
    }

    fn metadata(revision_conflict: bool, packet_sequence: u16) -> StreamingPcmChunkMetadata {
        StreamingPcmChunkMetadata {
            collector_instance_id: Some(41),
            segment_ordinal: 7,
            packet_sequence,
            emission_ordinal: u64::from(packet_sequence),
            emitted_range: StreamingPcmRange {
                start: u64::from(packet_sequence) * 100,
                end: u64::from(packet_sequence) * 100 + 100,
            },
            packet_revision: 3,
            packet_disposition: crate::embedded_audio::StreamingPcmChunkDisposition::Replacement,
            wire_payload_bytes: 100,
            declared_pcm_bytes: 100,
            expanded_pcm_bytes: 100,
            previous_emission_ordinal: Some(1),
            previous_emitted_range: Some(StreamingPcmRange { start: 0, end: 100 }),
            revision_conflict,
            metadata_incomplete: false,
        }
    }

    #[test]
    fn clean_handle_allows_only_its_current_owner() {
        let handle = SourceIntegrityHandle::for_candidate(9);
        assert_eq!(handle.owner(), SourceIntegrityOwner::Candidate(9));
        assert_eq!(
            qualify_session_integrity(&handle, SourceIntegrityOwner::Candidate(9)),
            SourceIntegrityQualification::Allowed
        );
        assert_eq!(
            qualify_session_integrity(&handle, SourceIntegrityOwner::Candidate(10)),
            SourceIntegrityQualification::OwnerMismatch
        );
    }

    #[test]
    fn conflict_is_sticky_and_keeps_first_evidence() {
        let handle = SourceIntegrityHandle::for_candidate(9);
        handle.observe_metadata(metadata(true, 12));
        handle.observe_metadata(metadata(true, 13));
        assert_eq!(handle.status(), SourceIntegrityStatus::RevisionConflict);
        assert_eq!(handle.conflict_count(), 2);
        assert_eq!(handle.first_conflict().unwrap().packet_sequence, 12);
        assert_eq!(
            qualify_session_integrity(&handle, SourceIntegrityOwner::Candidate(9)),
            SourceIntegrityQualification::SourceIntegrityBlocked
        );
    }

    #[test]
    fn non_conflict_metadata_does_not_change_status() {
        let handle = SourceIntegrityHandle::for_candidate(9);
        let mut incomplete = metadata(false, 12);
        incomplete.metadata_incomplete = true;
        handle.observe_metadata(incomplete);
        assert_eq!(handle.status(), SourceIntegrityStatus::NoKnownConflict);
        assert_eq!(handle.conflict_count(), 0);
        assert!(handle.first_conflict().is_none());
    }

    #[test]
    fn cloned_handle_shares_conflict_state() {
        let handle = SourceIntegrityHandle::for_candidate(9);
        let clone = handle.clone();
        clone.observe_metadata(metadata(true, 12));
        assert_eq!(handle.status(), SourceIntegrityStatus::RevisionConflict);
        assert_eq!(handle.conflict_count(), 1);
    }

    #[test]
    fn conflict_survives_candidate_to_session_rebind() {
        let handle = SourceIntegrityHandle::for_candidate(9);
        handle.observe_metadata(metadata(true, 12));
        let session_id = uuid::Uuid::new_v4();
        assert!(handle.rebind_owner(
            SourceIntegrityOwner::Candidate(9),
            SourceIntegrityOwner::Session(session_id),
        ));
        assert_eq!(
            qualify_session_integrity(&handle, SourceIntegrityOwner::Session(session_id)),
            SourceIntegrityQualification::SourceIntegrityBlocked
        );
        assert!(!handle.rebind_owner(
            SourceIntegrityOwner::Candidate(9),
            SourceIntegrityOwner::Session(uuid::Uuid::new_v4()),
        ));
    }

    #[test]
    fn independent_session_gets_independent_clean_handle() {
        let old = SourceIntegrityHandle::for_candidate(9);
        old.observe_metadata(metadata(true, 12));
        let new = SourceIntegrityHandle::for_candidate(10);
        assert_eq!(
            qualify_session_integrity(&old, SourceIntegrityOwner::Candidate(9)),
            SourceIntegrityQualification::SourceIntegrityBlocked
        );
        assert_eq!(
            qualify_session_integrity(&new, SourceIntegrityOwner::Candidate(10)),
            SourceIntegrityQualification::Allowed
        );
    }

    #[test]
    fn unified_decision_table_keeps_unknown_separate_from_blocked() {
        let owner_session_id = uuid::Uuid::new_v4();
        let (clean_fact, _) = admitted_audio(false);
        let mut clean_ledger = SourceAdmissionDependencyLedger::default();
        assert_eq!(
            qualify_active_source_integrity(
                SourceIntegrityOwner::Session(owner_session_id),
                SourceIntegrityOwner::Session(owner_session_id),
                &mut clean_ledger,
                None,
            ),
            SourceIntegrityDecision::allowed(
                SourceIntegrityEvidenceCompleteness::Complete,
                SourceIntegrityDecisionReason::NoConfirmedConflict,
                None,
            )
        );

        let mismatched = qualify_active_source_integrity(
            SourceIntegrityOwner::Candidate(8),
            SourceIntegrityOwner::Session(owner_session_id),
            &mut clean_ledger,
            None,
        );
        assert_eq!(
            mismatched.verdict,
            SourceIntegrityQualification::OwnerMismatch
        );
        assert_eq!(
            mismatched.reason,
            SourceIntegrityDecisionReason::OwnerMismatch
        );

        let (_, replaced_receipt) = admitted_audio(true);
        let mut blocked_ledger = SourceAdmissionDependencyLedger::default();
        blocked_ledger.record(dependency(
            SourceAdmissionOperationOwner::Session {
                session_id: owner_session_id,
            },
            SourceAdmissionUse::SessionBodyInput,
            clean_fact.clone(),
            replaced_receipt,
            Some(1),
            Some((0, 2)),
            Some((0, 2)),
        ));
        let blocked = qualify_active_source_integrity(
            SourceIntegrityOwner::Session(owner_session_id),
            SourceIntegrityOwner::Session(owner_session_id),
            &mut blocked_ledger,
            None,
        );
        assert_eq!(
            blocked.verdict,
            SourceIntegrityQualification::SourceIntegrityBlocked
        );
        assert_eq!(
            blocked.evidence_completeness,
            SourceIntegrityEvidenceCompleteness::Complete
        );
        assert_eq!(
            blocked.reason,
            SourceIntegrityDecisionReason::SessionBodyInputSuperseded
        );
        assert_eq!(blocked.first_evidence.unwrap().operation_id, Some(1));

        let (_, possible_receipt) = admitted_audio(true);
        let mut possible_ledger = SourceAdmissionDependencyLedger::default();
        possible_ledger.record(dependency(
            SourceAdmissionOperationOwner::Session {
                session_id: owner_session_id,
            },
            SourceAdmissionUse::SessionBodyInputPossible,
            clean_fact.clone(),
            possible_receipt,
            Some(2),
            Some((0, 2)),
            Some((0, 2)),
        ));
        let possible = qualify_active_source_integrity(
            SourceIntegrityOwner::Session(owner_session_id),
            SourceIntegrityOwner::Session(owner_session_id),
            &mut possible_ledger,
            None,
        );
        assert_eq!(possible.verdict, SourceIntegrityQualification::Allowed);
        assert_eq!(
            possible.evidence_completeness,
            SourceIntegrityEvidenceCompleteness::Unknown
        );
        assert_eq!(
            possible.reason,
            SourceIntegrityDecisionReason::SessionBodyInputPossibleSuperseded
        );

        let (_, candidate_receipt) = admitted_audio(true);
        let mut candidate_ledger = SourceAdmissionDependencyLedger::default();
        candidate_ledger.record(dependency(
            SourceAdmissionOperationOwner::Candidate { candidate_id: 9 },
            SourceAdmissionUse::CandidateGateInput,
            clean_fact,
            candidate_receipt,
            Some(3),
            Some((0, 2)),
            Some((0, 2)),
        ));
        let candidate = qualify_active_source_integrity(
            SourceIntegrityOwner::Session(owner_session_id),
            SourceIntegrityOwner::Session(owner_session_id),
            &mut candidate_ledger,
            None,
        );
        assert_eq!(candidate.verdict, SourceIntegrityQualification::Allowed);
        assert_eq!(
            candidate.evidence_completeness,
            SourceIntegrityEvidenceCompleteness::Unknown
        );
        assert_eq!(
            candidate.reason,
            SourceIntegrityDecisionReason::CandidateGateInputSuperseded
        );

        let actor_conflict = metadata(true, 12);
        let conflict_decision = qualify_active_source_integrity(
            SourceIntegrityOwner::Session(owner_session_id),
            SourceIntegrityOwner::Session(owner_session_id),
            &mut possible_ledger,
            Some(RevisionConflictEvidence::from_metadata(actor_conflict)),
        );
        assert_eq!(
            conflict_decision.verdict,
            SourceIntegrityQualification::SourceIntegrityBlocked
        );
        assert_eq!(
            conflict_decision.reason,
            SourceIntegrityDecisionReason::ActorRevisionConflict
        );
    }

    #[test]
    fn promotion_keeps_historical_operation_owner_and_isolates_new_session() {
        let session_id = uuid::Uuid::new_v4();
        let (fact, receipt) = admitted_audio(false);
        let mut promoted_ledger = SourceAdmissionDependencyLedger::default();
        promoted_ledger.record(dependency(
            SourceAdmissionOperationOwner::Candidate { candidate_id: 41 },
            SourceAdmissionUse::CandidateGateInput,
            fact.clone(),
            receipt,
            Some(11),
            Some((0, 2)),
            Some((0, 2)),
        ));
        let promoted = qualify_active_source_integrity(
            SourceIntegrityOwner::Session(session_id),
            SourceIntegrityOwner::Session(session_id),
            &mut promoted_ledger,
            None,
        );
        assert_eq!(promoted.verdict, SourceIntegrityQualification::Allowed);
        assert_eq!(
            promoted.evidence_completeness,
            SourceIntegrityEvidenceCompleteness::Complete
        );
        assert_eq!(
            promoted_ledger
                .active_dependencies()
                .next()
                .expect("promoted dependency")
                .operation_owner,
            SourceAdmissionOperationOwner::Candidate { candidate_id: 41 }
        );

        let (_, old_receipt) = admitted_audio(true);
        let mut old_ledger = SourceAdmissionDependencyLedger::default();
        old_ledger.record(dependency(
            SourceAdmissionOperationOwner::Session { session_id },
            SourceAdmissionUse::SessionBodyInput,
            fact,
            old_receipt,
            Some(12),
            Some((0, 2)),
            Some((0, 2)),
        ));
        let new_session_id = uuid::Uuid::new_v4();
        let (new_fact, new_receipt) = admitted_audio(false);
        let mut new_ledger = SourceAdmissionDependencyLedger::default();
        new_ledger.record(dependency(
            SourceAdmissionOperationOwner::Session {
                session_id: new_session_id,
            },
            SourceAdmissionUse::SessionBodyInput,
            new_fact,
            new_receipt,
            Some(1),
            Some((0, 2)),
            Some((0, 2)),
        ));
        assert_eq!(
            qualify_active_source_integrity(
                SourceIntegrityOwner::Session(new_session_id),
                SourceIntegrityOwner::Session(new_session_id),
                &mut new_ledger,
                None,
            )
            .verdict,
            SourceIntegrityQualification::Allowed
        );
        assert_eq!(
            qualify_active_source_integrity(
                SourceIntegrityOwner::Session(session_id),
                SourceIntegrityOwner::Session(session_id),
                &mut old_ledger,
                None,
            )
            .verdict,
            SourceIntegrityQualification::SourceIntegrityBlocked
        );
    }

    #[test]
    fn active_reference_survives_projection_rotation_and_still_blocks() {
        let session_id = uuid::Uuid::new_v4();
        let (fact, receipt) = admitted_audio(true);
        let mut ledger = SourceAdmissionDependencyLedger::default();
        ledger.record(dependency(
            SourceAdmissionOperationOwner::Session { session_id },
            SourceAdmissionUse::SessionBodyInput,
            fact.clone(),
            receipt.clone(),
            Some(1),
            Some((0, 2)),
            Some((0, 2)),
        ));
        for operation_id in 2..=4_097_u64 {
            ledger.record(dependency(
                SourceAdmissionOperationOwner::Session { session_id },
                SourceAdmissionUse::SessionBodyInput,
                fact.clone(),
                receipt.clone(),
                Some(operation_id),
                Some((operation_id, operation_id + 2)),
                Some((operation_id, operation_id + 2)),
            ));
        }
        let active = ledger
            .active_dependencies()
            .next()
            .expect("active dependency after projection rotation");
        assert!(active.tracking_incomplete);
        assert!(!active.non_projection_incomplete);
        let decision = qualify_active_source_integrity(
            SourceIntegrityOwner::Session(session_id),
            SourceIntegrityOwner::Session(session_id),
            &mut ledger,
            None,
        );
        assert_eq!(
            decision.verdict,
            SourceIntegrityQualification::SourceIntegrityBlocked
        );
        assert_eq!(
            decision.reason,
            SourceIntegrityDecisionReason::SessionBodyInputSuperseded
        );
    }

    #[test]
    fn confirmed_conflict_is_sticky_after_tracking_degrades() {
        let session_id = uuid::Uuid::new_v4();
        let (fact, receipt) = admitted_audio(true);
        let mut ledger = SourceAdmissionDependencyLedger::default();
        ledger.record(dependency(
            SourceAdmissionOperationOwner::Session { session_id },
            SourceAdmissionUse::SessionBodyInput,
            fact,
            receipt,
            Some(1),
            Some((0, 2)),
            Some((0, 2)),
        ));
        let first = qualify_active_source_integrity(
            SourceIntegrityOwner::Session(session_id),
            SourceIntegrityOwner::Session(session_id),
            &mut ledger,
            None,
        );
        assert_eq!(
            first.verdict,
            SourceIntegrityQualification::SourceIntegrityBlocked
        );

        ledger
            .active_dependencies_mut()
            .next()
            .expect("active body dependency")
            .tracking_incomplete = true;
        ledger.mark_incomplete();
        let second = qualify_active_source_integrity(
            SourceIntegrityOwner::Session(session_id),
            SourceIntegrityOwner::Session(session_id),
            &mut ledger,
            Some(RevisionConflictEvidence::from_metadata(metadata(true, 13))),
        );
        assert_eq!(second, first);
        assert_eq!(
            second.evidence_completeness,
            SourceIntegrityEvidenceCompleteness::Complete
        );
    }

    #[test]
    fn unconfirmed_incomplete_witness_is_unknown_not_complete() {
        let session_id = uuid::Uuid::new_v4();
        let (fact, receipt, mut collector) = admitted_audio_with_collector();
        for sequence in 1..=4_096_u16 {
            collector
                .handle_notification(
                    &crate::embedded_audio::build_audio_data_notification(7, sequence, &[3, 4])
                        .expect("filler audio"),
                )
                .expect("filler audio event");
        }
        assert!(receipt.witness().metadata_incomplete);

        let mut ledger = SourceAdmissionDependencyLedger::default();
        ledger.record(dependency(
            SourceAdmissionOperationOwner::Session { session_id },
            SourceAdmissionUse::SessionBodyInput,
            fact,
            receipt,
            Some(1),
            Some((0, 2)),
            Some((0, 2)),
        ));
        let decision = qualify_active_source_integrity(
            SourceIntegrityOwner::Session(session_id),
            SourceIntegrityOwner::Session(session_id),
            &mut ledger,
            None,
        );
        assert_eq!(decision.verdict, SourceIntegrityQualification::Allowed);
        assert_eq!(
            decision.evidence_completeness,
            SourceIntegrityEvidenceCompleteness::Unknown
        );
        assert_eq!(
            decision.reason,
            SourceIntegrityDecisionReason::IncompleteSourceDependency
        );
    }

    #[test]
    fn first_evidence_retains_full_identity_and_is_not_overwritten() {
        let session_id = uuid::Uuid::new_v4();
        let (fact_a, receipt_a) = admitted_audio(true);
        let (fact_b, receipt_b) = admitted_audio(true);
        let mut ledger_a = SourceAdmissionDependencyLedger::default();
        ledger_a.record(dependency(
            SourceAdmissionOperationOwner::Session { session_id },
            SourceAdmissionUse::SessionBodyInput,
            fact_a.clone(),
            receipt_a,
            Some(1),
            Some((0, 2)),
            Some((0, 2)),
        ));
        let first = qualify_active_source_integrity(
            SourceIntegrityOwner::Session(session_id),
            SourceIntegrityOwner::Session(session_id),
            &mut ledger_a,
            None,
        );
        let evidence = first.first_evidence.expect("first conflict evidence");
        assert_eq!(evidence.capture_generation, Some(1));
        assert_eq!(evidence.collector_instance_id, fact_a.collector_instance_id);
        assert_eq!(evidence.segment_ordinal, Some(7));
        assert_eq!(evidence.packet_revision, Some(3));
        assert_eq!(evidence.emission_ordinal, Some(0));
        assert_eq!(
            evidence.emitted_range,
            Some(StreamingPcmRange { start: 0, end: 100 })
        );

        let mut ledger_b = SourceAdmissionDependencyLedger::default();
        ledger_b.record(dependency(
            SourceAdmissionOperationOwner::Session { session_id },
            SourceAdmissionUse::SessionBodyInput,
            fact_b.clone(),
            receipt_b,
            Some(1),
            Some((0, 2)),
            Some((0, 2)),
        ));
        let other = qualify_active_source_integrity(
            SourceIntegrityOwner::Session(session_id),
            SourceIntegrityOwner::Session(session_id),
            &mut ledger_b,
            None,
        );
        assert_ne!(
            evidence.collector_instance_id,
            other
                .first_evidence
                .expect("other conflict evidence")
                .collector_instance_id
        );

        let repeated = qualify_active_source_integrity(
            SourceIntegrityOwner::Session(session_id),
            SourceIntegrityOwner::Session(session_id),
            &mut ledger_a,
            Some(RevisionConflictEvidence::from_metadata(metadata(true, 99))),
        );
        assert_eq!(repeated, first);
    }
}
