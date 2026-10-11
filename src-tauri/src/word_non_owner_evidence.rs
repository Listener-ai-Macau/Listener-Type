//! Experimental, source-bound corroboration for an individual provider word.
//!
//! This module is test-only until independent short/quiet and foreign holdouts
//! validate its false-deletion rate. A Candidate is NOT WordIdentity::Other and
//! cannot change endpoints, enrollment, or production insertion authority.

use crate::speech_decision_kernel::TranscriptSpeakerEvidence;
use crate::word_authority::AudioSource;

type Range = (u64, u64);

pub(crate) struct OwnerTrack<'a> {
    pub source: AudioSource,
    /// Filled only after independent, qualified matching to the frozen bank.
    /// Multiple or conflicting bindings remain unresolved in this experiment.
    pub unique_bound_channel: Option<u8>,
    pub binding_observed_audio_ms: u64,
    pub settled_through_ms: u64,
    pub native_core: &'a [Range],
    /// Conservative SDK presentation spans guard the complement gap. They do
    /// not themselves supply positive source-word identity.
    pub guarded_spans: &'a [Range],
}

pub(crate) struct GapObservation {
    pub source: AudioSource,
    pub range: Range,
    /// Score/peak/energy qualification from the unchanged CAM transcript rule.
    pub raw_transcript_evidence: TranscriptSpeakerEvidence,
    /// Completed coverage of this exact source and range, not an upper bound.
    pub resolved_speech_ms: Option<usize>,
    pub vad_resolved_through_ms: u64,
    pub cam_observed_audio_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PendingReason {
    MissingWordCoordinates,
    SourceMismatch,
    UnboundOwner,
    UnsettledEvidence,
    InvalidEvidence,
    OutsideGap,
    OwnerConflict,
    NoCompletedSpeech,
    NoIndependentHint,
    NoOwnerActivity,
    CompetingActivity,
}

/// A word's timestamp is a recognition interval, not a point observation.
/// Core overlap can support its identity without treating SDK padding as
/// speech. Concurrent tracks still require an independent separated source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ExperimentalOwnerWordCandidate {
    pub source: AudioSource,
    pub word: Range,
    pub owner_channel: u8,
    pub binding_observed_audio_ms: u64,
}

pub(crate) fn corroborate_owner_word(
    source: AudioSource,
    word: Option<Range>,
    decision_audio_ms: u64,
    owner: &OwnerTrack<'_>,
    competing_core: &[Range],
    qualified_negative_ranges: &[Range],
) -> Result<ExperimentalOwnerWordCandidate, PendingReason> {
    let word = word.filter(|range| valid(*range)).ok_or(PendingReason::MissingWordCoordinates)?;
    if source != owner.source { return Err(PendingReason::SourceMismatch); }
    let channel = owner.unique_bound_channel.filter(|c| (1..=8).contains(c))
        .ok_or(PendingReason::UnboundOwner)?;
    if owner.native_core.iter().chain(competing_core).chain(qualified_negative_ranges)
        .any(|r| !valid(*r)) { return Err(PendingReason::InvalidEvidence); }
    if word.1 > decision_audio_ms || word.1 > owner.settled_through_ms
        || owner.binding_observed_audio_ms > decision_audio_ms {
        return Err(PendingReason::UnsettledEvidence);
    }
    if competing_core.iter().chain(qualified_negative_ranges).any(|r| overlaps(*r, word)) {
        return Err(PendingReason::CompetingActivity);
    }
    if !owner.native_core.iter().any(|r| overlaps(*r, word)) {
        return Err(PendingReason::NoOwnerActivity);
    }
    Ok(ExperimentalOwnerWordCandidate { source, word, owner_channel: channel,
        binding_observed_audio_ms: owner.binding_observed_audio_ms })
}

/// Deliberately distinct from production deletion permission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ExperimentalWordCandidate {
    pub source: AudioSource,
    pub word: Range,
    pub gap: Range,
    pub owner_channel: u8,
    pub bounded_hint: TranscriptSpeakerEvidence,
    pub resolved_speech_ms: usize,
}

pub(crate) fn corroborate_word(
    source: AudioSource,
    word: Option<Range>,
    decision_audio_ms: u64,
    owner: &OwnerTrack<'_>,
    gap: &GapObservation,
) -> Result<ExperimentalWordCandidate, PendingReason> {
    let word = word.filter(|range| valid(*range)).ok_or(PendingReason::MissingWordCoordinates)?;
    if source != owner.source || source != gap.source {
        return Err(PendingReason::SourceMismatch);
    }
    let channel = owner.unique_bound_channel.filter(|c| (1..=8).contains(c))
        .ok_or(PendingReason::UnboundOwner)?;
    if !valid(gap.range)
        || owner.native_core.iter().chain(owner.guarded_spans).any(|r| !valid(*r))
    {
        return Err(PendingReason::InvalidEvidence);
    }
    if word.1 > decision_audio_ms || gap.range.1 > decision_audio_ms
        || owner.binding_observed_audio_ms > decision_audio_ms
        || gap.cam_observed_audio_ms > decision_audio_ms
        || gap.cam_observed_audio_ms < gap.range.1
        || owner.settled_through_ms < word.1.max(gap.range.1)
        || gap.vad_resolved_through_ms < gap.range.1
        || gap.vad_resolved_through_ms > decision_audio_ms
    {
        return Err(PendingReason::UnsettledEvidence);
    }
    if !overlaps(word, gap.range) {
        return Err(PendingReason::OutsideGap);
    }
    // A hint from a gap may not erase an Owner-bearing boundary word. A caller
    // also may not label an arbitrary context as a complement gap.
    if owner.native_core.iter().any(|r| overlaps(*r, word) || overlaps(*r, gap.range))
        || owner.guarded_spans.iter().any(|r| overlaps(*r, gap.range))
    {
        return Err(PendingReason::OwnerConflict);
    }
    let speech_ms = gap.resolved_speech_ms.ok_or(PendingReason::NoCompletedSpeech)?;
    if speech_ms as u64 > gap.range.1 - gap.range.0 {
        return Err(PendingReason::InvalidEvidence);
    }
    let bounded_hint = gap.raw_transcript_evidence.constrained_by_resolved_speech(speech_ms);
    if bounded_hint == TranscriptSpeakerEvidence::Inconclusive {
        return Err(PendingReason::NoIndependentHint);
    }
    Ok(ExperimentalWordCandidate {
        source, word, gap: gap.range, owner_channel: channel, bounded_hint,
        resolved_speech_ms: speech_ms,
    })
}

fn valid((start,end): Range) -> bool { start < end }
fn overlaps(a: Range, b: Range) -> bool { a.0 < b.1 && b.0 < a.1 }

#[cfg(test)]
mod tests {
    use super::*;
    const SOURCE: AudioSource = AudioSource { stream: 1, generation: 2 };

    #[test]
    fn owner_boundary_word_uses_real_core_overlap_and_never_display_padding() {
        // Independent Owner-only holdout: real provider intervals straddle
        // short core gaps. Midpoint-only projection would stop valid text.
        let owner = OwnerTrack { source: SOURCE, unique_bound_channel: Some(1),
            binding_observed_audio_ms: 3000, settled_through_ms: 5000,
            native_core: &[(330,1270),(1380,2000),(2080,2630)],
            guarded_spans: &[(101,2709)] };
        for word in [(1280,1440),(2000,2120)] {
            assert!(corroborate_owner_word(SOURCE,Some(word),5000,&owner,&[],&[]).is_ok());
        }
        for word in [(100,200),(1270,1380),(2000,2080)] {
            assert_eq!(corroborate_owner_word(SOURCE,Some(word),5000,&owner,&[],&[]),
                Err(PendingReason::NoOwnerActivity));
        }
    }

    #[test]
    fn overlap_negative_or_future_binding_cannot_authorize_owner_word() {
        let owner = owner();
        let word = Some((300,500));
        for (competing,negative) in [(&[(499,550)][..],&[][..]),(&[][..],&[(499,550)][..])] {
            assert_eq!(corroborate_owner_word(SOURCE,word,3000,&owner,competing,negative),
                Err(PendingReason::CompetingActivity));
        }
        assert_eq!(corroborate_owner_word(SOURCE,word,500,&owner,&[],&[]),
            Err(PendingReason::UnsettledEvidence));
        assert_eq!(corroborate_owner_word(AudioSource { generation: 3, ..SOURCE },word,3000,&owner,&[],&[]),
            Err(PendingReason::SourceMismatch));
    }
    fn owner() -> OwnerTrack<'static> {
        OwnerTrack { source: SOURCE, unique_bound_channel: Some(1),
            binding_observed_audio_ms: 900, settled_through_ms: 3000,
            native_core: &[(100,900),(2300,2900)], guarded_spans: &[(0,1000),(2200,3000)] }
    }
    fn gap() -> GapObservation {
        GapObservation { source: SOURCE, range: (1000,2200),
            raw_transcript_evidence: TranscriptSpeakerEvidence::HardNonTarget,
            resolved_speech_ms: Some(700), vad_resolved_through_ms: 3000,
            cam_observed_audio_ms: 2400 }
    }
    fn candidate(owner: &OwnerTrack<'_>, gap: &GapObservation) -> Result<ExperimentalWordCandidate,PendingReason> {
        corroborate_word(SOURCE, Some((1100,1400)), 3000, owner, gap)
    }

    #[test]
    fn completed_short_speech_downgrades_the_hint_without_granting_global_identity() {
        for duration in [300,450,599] {
            let mut gap = gap(); gap.resolved_speech_ms = Some(duration);
            assert_eq!(candidate(&owner(), &gap).unwrap().bounded_hint, TranscriptSpeakerEvidence::ForeignTailHint);
        }
        let mut gap = gap(); gap.resolved_speech_ms = Some(299);
        assert_eq!(candidate(&owner(), &gap), Err(PendingReason::NoIndependentHint));
        gap.resolved_speech_ms = None;
        assert_eq!(candidate(&owner(), &gap), Err(PendingReason::NoCompletedSpeech));
        gap.resolved_speech_ms = Some(700);
        gap.raw_transcript_evidence = TranscriptSpeakerEvidence::ForeignTailHint;
        assert_eq!(candidate(&owner(), &gap).unwrap().bounded_hint, TranscriptSpeakerEvidence::ForeignTailHint);
    }

    #[test]
    fn owner_core_at_any_part_of_a_boundary_word_prevents_a_negative_candidate() {
        let mut owner = owner(); owner.native_core = &[(100,990),(2300,2900)];
        assert_eq!(corroborate_word(SOURCE,Some((950,1150)),3000,&owner,&gap()), Err(PendingReason::OwnerConflict));
        owner = self::owner();
        assert!(corroborate_word(SOURCE,Some((950,1150)),3000,&owner,&gap()).is_ok(),
            "display collar alone must not masquerade as native Owner activity");
        let mut bad_gap = gap(); bad_gap.range = (900,2200);
        assert_eq!(candidate(&owner,&bad_gap),Err(PendingReason::OwnerConflict));
    }

    #[test]
    fn a_gap_or_anonymous_channel_without_independent_evidence_stays_pending() {
        let mut owner = owner(); owner.unique_bound_channel = None;
        assert_eq!(candidate(&owner,&gap()), Err(PendingReason::UnboundOwner));
        let mut gap = gap(); gap.raw_transcript_evidence = TranscriptSpeakerEvidence::Inconclusive;
        assert_eq!(candidate(&self::owner(),&gap), Err(PendingReason::NoIndependentHint));
        gap = self::gap(); gap.resolved_speech_ms = Some(0);
        assert_eq!(candidate(&self::owner(),&gap), Err(PendingReason::NoIndependentHint));
    }

    #[test]
    fn future_or_different_capture_evidence_cannot_authorize_a_word() {
        let mut gap = gap(); gap.source.generation += 1;
        assert_eq!(candidate(&owner(),&gap), Err(PendingReason::SourceMismatch));
        gap = self::gap(); gap.cam_observed_audio_ms = 3100;
        assert_eq!(candidate(&owner(),&gap), Err(PendingReason::UnsettledEvidence));
        gap = self::gap(); gap.vad_resolved_through_ms = 2100;
        assert_eq!(candidate(&owner(),&gap), Err(PendingReason::UnsettledEvidence));
        assert_eq!(corroborate_word(SOURCE,None,3000,&owner(),&self::gap()), Err(PendingReason::MissingWordCoordinates));
        let mut owner = owner(); owner.settled_through_ms = 2000;
        assert_eq!(candidate(&owner,&self::gap()), Err(PendingReason::UnsettledEvidence));
    }
}
