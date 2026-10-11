//! Test-only source-word certificates. No model scoring or identity thresholds
//! are fitted here. Anonymous channels and short negative hints never become
//! production permissions; the acoustic producer still needs joint validation.

use super::{project_timing, Projection, WordDecision, WordIdentity};
use crate::speaker_channel_binding::{
    AnonymousSampleSpan, MillisecondRange, OwnerChannelBinding, SampleRange, SourceGeneration,
    VerifiedWakeOwnerSampleBinding,
};
use crate::speaker_verification::{
    SessionSpeakerClassification, SESSION_SPEAKER_MIN_NON_TARGET_MS,
};
use crate::transcript_boundary::TimedTranscript;
use std::time::Instant;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SourceWordBinding {
    coordinates: VerifiedWakeOwnerSampleBinding,
}

impl SourceWordBinding {
    pub(crate) fn from_verified_wake(binding: VerifiedWakeOwnerSampleBinding) -> Self {
        Self {
            coordinates: binding,
        }
    }

    /// The caller must obtain Bound from the existing source-local binder's
    /// completed, frozen protected-bank observations. A raw ASR/native ID or
    /// uncertain window is not a substitute for that qualified binding.
    pub(crate) fn from_qualified_channel(
        source: SourceGeneration,
        binding: OwnerChannelBinding,
        body_origin_sample: u64,
        observed_at_input_sample: u64,
    ) -> Result<Self, SourceWordPendingReason> {
        let OwnerChannelBinding::Bound { channel } = binding else {
            return Err(SourceWordPendingReason::UnboundOwner);
        };
        if !(1..=8).contains(&channel) || body_origin_sample > observed_at_input_sample {
            return Err(SourceWordPendingReason::InvalidEvidence);
        }
        // The fields are common source/channel/time coordinates. No verified
        // wake claim is made by this separate protected-window construction.
        Ok(Self {
            coordinates: VerifiedWakeOwnerSampleBinding {
                source,
                channel,
                body_origin_sample,
                valid_after_input_sample: observed_at_input_sample,
            },
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SourceWordPendingReason {
    SourceMismatch,
    UnboundOwner,
    InvalidEvidence,
    FutureEvidence,
    RewrittenSettledHistory,
    BindingChanged,
    OwnerConflict,
    NoCompletedOtherIdentity,
}

#[derive(Clone, Copy)]
struct NativeReceipt {
    settled: u64,
    input: u64,
    at: Instant,
}

/// Exact completed original-source CAM/VAD facts supplied by their producers.
/// The range is their whole scored waveform, not a smaller picked text cell.
pub(crate) struct OtherSourceObservation {
    pub(crate) source: SourceGeneration,
    pub(crate) range: SampleRange,
    pub(crate) classification: SessionSpeakerClassification,
    pub(crate) signal_quality_sufficient: bool,
    pub(crate) resolved_speech_ms: Option<usize>,
    pub(crate) vad_resolved_through_sample: u64,
    pub(crate) observed_at_input_sample: u64,
    pub(crate) observed_at: Instant,
}

struct OtherCertificate {
    range: SampleRange,
    input: u64,
    at: Instant,
}

pub(crate) struct CapturedWordAuthority {
    source: SourceGeneration,
    binding: Option<SourceWordBinding>,
    binding_at: Option<Instant>,
    core: Vec<AnonymousSampleSpan>,
    receipts: Vec<NativeReceipt>,
    latest: Option<NativeReceipt>,
    other: Vec<OtherCertificate>,
    unavailable: bool,
}

impl CapturedWordAuthority {
    pub(crate) fn new(source: SourceGeneration) -> Self {
        Self {
            source,
            binding: None,
            binding_at: None,
            core: Vec::new(),
            receipts: Vec::new(),
            latest: None,
            other: Vec::new(),
            unavailable: false,
        }
    }

    /// A serialized snapshot of the actual source-bound, settled native core.
    /// Failed input never updates the old ledger or supplies a newer clock.
    pub(crate) fn observe_native(
        &mut self,
        source: SourceGeneration,
        binding: SourceWordBinding,
        core: &[AnonymousSampleSpan],
        settled: u64,
        input: u64,
        at: Instant,
    ) -> Result<(), SourceWordPendingReason> {
        if self.unavailable {
            return Err(SourceWordPendingReason::InvalidEvidence);
        }
        if source != self.source || binding.coordinates.source != self.source {
            return Err(SourceWordPendingReason::SourceMismatch);
        }
        if settled > input
            || binding.coordinates.valid_after_input_sample > input
            || binding.coordinates.body_origin_sample > input
        {
            return Err(SourceWordPendingReason::FutureEvidence);
        }
        if !(1..=8).contains(&binding.coordinates.channel)
            || core.iter().any(|c| {
                !(1..=8).contains(&c.channel)
                    || c.range.start >= c.range.end
                    || c.range.end > settled
            })
        {
            return Err(SourceWordPendingReason::InvalidEvidence);
        }
        if self.binding.is_some_and(|old| old != binding) {
            return Err(SourceWordPendingReason::BindingChanged);
        }
        if let Some(previous) = self.latest {
            if input < previous.input || settled < previous.settled || at < previous.at {
                return Err(SourceWordPendingReason::FutureEvidence);
            }
            if canonical_core(core, previous.settled)
                != canonical_core(&self.core, previous.settled)
            {
                return Err(SourceWordPendingReason::RewrittenSettledHistory);
            }
        }
        self.binding.get_or_insert(binding);
        self.binding_at.get_or_insert(at);
        self.core = canonical_core(core, settled);
        self.latest = Some(NativeReceipt { settled, input, at });
        // Repeated snapshots and input-only growth do not rejuvenate the
        // permission time of a word already inside the settled ledger.
        if self
            .receipts
            .last()
            .is_none_or(|last| last.settled < settled)
        {
            self.receipts.push(NativeReceipt { settled, input, at });
        }
        Ok(())
    }

    pub(crate) fn publish_other(
        &mut self,
        observation: OtherSourceObservation,
    ) -> Result<(), SourceWordPendingReason> {
        if observation.source != self.source {
            return Err(SourceWordPendingReason::SourceMismatch);
        }
        let last = self
            .latest
            .as_ref()
            .ok_or(SourceWordPendingReason::UnboundOwner)?;
        if self.unavailable || observation.range.start >= observation.range.end {
            return Err(SourceWordPendingReason::InvalidEvidence);
        }
        if observation.range.end > last.settled
            || observation.vad_resolved_through_sample < observation.range.end
            || observation.vad_resolved_through_sample > observation.observed_at_input_sample
            || observation.observed_at_input_sample < observation.range.end
            || observation.observed_at_input_sample > last.input
        {
            return Err(SourceWordPendingReason::FutureEvidence);
        }
        let speech = observation
            .resolved_speech_ms
            .ok_or(SourceWordPendingReason::NoCompletedOtherIdentity)?;
        if !matches!(
            observation.classification,
            SessionSpeakerClassification::NonTarget { .. }
        ) || !observation.signal_quality_sufficient
            || speech < SESSION_SPEAKER_MIN_NON_TARGET_MS
        {
            return Err(SourceWordPendingReason::NoCompletedOtherIdentity);
        }
        if u64::try_from(speech)
            .ok()
            .and_then(|ms| ms.checked_mul(16))
            .is_none_or(|samples| samples > observation.range.end - observation.range.start)
        {
            return Err(SourceWordPendingReason::InvalidEvidence);
        }
        let channel = self.binding.unwrap().coordinates.channel;
        if self
            .core
            .iter()
            .any(|c| c.channel == channel && overlaps(c.range, observation.range))
        {
            return Err(SourceWordPendingReason::OwnerConflict);
        }
        // Keep the first complete certificate. Duplicate publications cannot
        // change the evidence age or stand in for a new audio occurrence.
        if !self.other.iter().any(|old| old.range == observation.range) {
            self.other.push(OtherCertificate {
                range: observation.range,
                input: observation.observed_at_input_sample,
                at: observation.observed_at,
            });
        }
        Ok(())
    }

    pub(crate) fn mark_unavailable(&mut self) {
        self.unavailable = true;
    }

    pub(crate) fn identity(
        &self,
        source: SourceGeneration,
        start_ms: u64,
        end_ms: u64,
        decision_input: u64,
        decision_at: Instant,
    ) -> WordIdentity {
        self.decision(source, start_ms, end_ms, decision_input, decision_at)
            .identity
    }

    fn decision(
        &self,
        source: SourceGeneration,
        start_ms: u64,
        end_ms: u64,
        decision_input: u64,
        decision_at: Instant,
    ) -> WordDecision {
        if self.unavailable || source != self.source {
            return WordDecision::pending();
        }
        let Some(binding) = self.binding else {
            return WordDecision::pending();
        };
        let Some(binding_at) = self.binding_at.filter(|at| *at <= decision_at) else {
            return WordDecision::pending();
        };
        let Ok(word) = binding.coordinates.map_body_range(
            source,
            MillisecondRange { start_ms, end_ms },
            decision_input,
        ) else {
            return WordDecision::pending();
        };
        let Some(receipt) = self
            .receipts
            .iter()
            .find(|r| r.settled >= word.end && r.input <= decision_input && r.at <= decision_at)
        else {
            return WordDecision::pending();
        };
        let owner = self
            .core
            .iter()
            .any(|c| c.channel == binding.coordinates.channel && overlaps(c.range, word));
        let competing = self
            .core
            .iter()
            .any(|c| c.channel != binding.coordinates.channel && overlaps(c.range, word));
        let other = self
            .other
            .iter()
            .filter(|c| c.input <= decision_input && c.at <= decision_at && overlaps(c.range, word))
            .max_by_key(|c| c.at);
        if owner && (competing || other.is_some()) {
            return WordDecision::pending();
        }
        if owner {
            return WordDecision {
                identity: WordIdentity::Owner,
                observed_at: Some(binding_at.max(receipt.at)),
            };
        }
        if let Some(other) = other {
            return WordDecision {
                identity: WordIdentity::Other,
                observed_at: Some(binding_at.max(receipt.at).max(other.at)),
            };
        }
        WordDecision::pending()
    }

    pub(crate) fn project(
        &self,
        source: SourceGeneration,
        timing: &TimedTranscript,
        visual: bool,
        decision_input: u64,
        decision_at: Instant,
    ) -> Projection {
        project_timing(timing, visual, |start, end| {
            self.decision(source, start, end, decision_input, decision_at)
        })
    }
}

fn overlaps(a: SampleRange, b: SampleRange) -> bool {
    a.start < b.end && b.start < a.end
}

fn canonical_core(spans: &[AnonymousSampleSpan], through: u64) -> Vec<AnonymousSampleSpan> {
    let mut spans: Vec<_> = spans
        .iter()
        .filter_map(|c| {
            let end = c.range.end.min(through);
            (c.range.start < end).then_some(AnonymousSampleSpan {
                channel: c.channel,
                range: SampleRange {
                    start: c.range.start,
                    end,
                },
            })
        })
        .collect();
    spans.sort_by_key(|c| (c.channel, c.range.start, c.range.end));
    let mut result: Vec<AnonymousSampleSpan> = Vec::new();
    for span in spans {
        if let Some(last) = result.last_mut() {
            if last.channel == span.channel && span.range.start <= last.range.end {
                last.range.end = last.range.end.max(span.range.end);
                continue;
            }
        }
        result.push(span);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript_boundary::TimedTranscriptWord;
    use std::time::Duration;
    const SOURCE: SourceGeneration = SourceGeneration {
        source_id: 1,
        capture_id: 2,
        generation: 3,
    };
    fn span(channel: u8, start: u64, end: u64) -> AnonymousSampleSpan {
        AnonymousSampleSpan {
            channel,
            range: SampleRange {
                start: start * 16,
                end: end * 16,
            },
        }
    }
    fn binding() -> SourceWordBinding {
        SourceWordBinding::from_qualified_channel(
            SOURCE,
            OwnerChannelBinding::Bound { channel: 1 },
            0,
            800 * 16,
        )
        .unwrap()
    }
    fn authority(at: Instant) -> CapturedWordAuthority {
        let mut result = CapturedWordAuthority::new(SOURCE);
        result
            .observe_native(
                SOURCE,
                binding(),
                &[span(1, 100, 900), span(1, 2300, 2900)],
                3000 * 16,
                3200 * 16,
                at,
            )
            .unwrap();
        result
    }
    fn timing() -> TimedTranscript {
        TimedTranscript {
            text: "本人前句。旁人句。本人返回。".into(),
            words: [
                ("本人前句", 300, 700),
                ("旁人句", 1100, 1400),
                ("本人返回", 2300, 2700),
            ]
            .iter()
            .map(|(text, start, end)| TimedTranscriptWord {
                text: (*text).into(),
                start_ms: Some(*start),
                end_ms: Some(*end),
            })
            .collect(),
        }
    }
    fn negative(at: Instant, speech: Option<usize>) -> OtherSourceObservation {
        OtherSourceObservation {
            source: SOURCE,
            range: SampleRange {
                start: 1000 * 16,
                end: 2200 * 16,
            },
            classification: SessionSpeakerClassification::NonTarget { score: 0.02 },
            signal_quality_sufficient: true,
            resolved_speech_ms: speech,
            vad_resolved_through_sample: 3000 * 16,
            observed_at_input_sample: 3200 * 16,
            observed_at: at,
        }
    }

    #[test]
    fn two_real_boundary_words_use_core_overlap_and_mixed_words_stay_pending() {
        let at = Instant::now();
        let mut ledger = CapturedWordAuthority::new(SOURCE);
        let core = [span(1, 330, 1270), span(1, 1380, 2000), span(1, 2080, 2630)];
        ledger
            .observe_native(SOURCE, binding(), &core, 3000 * 16, 3200 * 16, at)
            .unwrap();
        // Independent quiet-receipt-resume provider intervals. Their midpoint
        // lies in a raw core gap, but each has actual bound-channel support.
        for (start, end) in [(1280, 1440), (2000, 2120)] {
            assert_eq!(
                ledger.identity(SOURCE, start, end, 3200 * 16, at),
                WordIdentity::Owner
            );
        }
        for (start, end) in [(1270, 1380), (2000, 2080)] {
            assert_eq!(
                ledger.identity(SOURCE, start, end, 3200 * 16, at),
                WordIdentity::Pending
            );
        }
        let mut mixed = CapturedWordAuthority::new(SOURCE);
        let mut concurrent = core.to_vec();
        concurrent.push(span(2, 1400, 1500));
        mixed
            .observe_native(SOURCE, binding(), &concurrent, 3000 * 16, 3200 * 16, at)
            .unwrap();
        assert_eq!(
            mixed.identity(SOURCE, 1280, 1440, 3200 * 16, at),
            WordIdentity::Pending
        );
        assert_eq!(
            mixed.identity(SOURCE, 2000, 2120, 3200 * 16, at),
            WordIdentity::Owner
        );
    }

    #[test]
    fn preview_live_and_final_share_pending_and_completed_other_permissions() {
        let at = Instant::now();
        let later = at + Duration::from_secs(1);
        let mut ledger = authority(at);
        let timing = timing();
        let early = ledger.project(SOURCE, &timing, false, 3200 * 16, at);
        assert_eq!(early.timing.text, "本人前句。");
        assert!(early.unresolved);
        assert!(!early.excluded_other);
        assert_eq!(
            ledger
                .project(SOURCE, &timing, true, 3200 * 16, at)
                .timing
                .text,
            timing.text
        );
        ledger.publish_other(negative(later, Some(1000))).unwrap();
        assert_eq!(
            ledger
                .project(SOURCE, &timing, false, 3200 * 16, at)
                .timing
                .text,
            "本人前句。"
        );
        let live = ledger.project(SOURCE, &timing, false, 3200 * 16, later);
        let final_view = ledger.project(SOURCE, &timing, false, 3200 * 16, later);
        let visual = ledger.project(SOURCE, &timing, true, 3200 * 16, later);
        assert_eq!(live.timing.text, "本人前句。本人返回。");
        assert_eq!(live.timing.text, final_view.timing.text);
        assert_eq!(visual.timing.text, final_view.timing.text);
        assert_eq!(live.timing.words.len(), 2);
        assert!(!live.unresolved);
        assert!(live.excluded_other);
        assert_eq!(live.updated_at, Some(later));
    }

    #[test]
    fn actual_367ms_negative_gap_cannot_be_promoted_from_raw_quality() {
        let at = Instant::now();
        let mut ledger = authority(at);
        for speech in [None, Some(367), Some(999)] {
            assert_eq!(
                ledger.publish_other(negative(at, speech)),
                Err(SourceWordPendingReason::NoCompletedOtherIdentity)
            );
        }
        assert_eq!(
            ledger.identity(SOURCE, 1100, 1400, 3200 * 16, at),
            WordIdentity::Pending
        );
        let mut unqualified = negative(at, Some(1000));
        unqualified.signal_quality_sufficient = false;
        assert_eq!(
            ledger.publish_other(unqualified),
            Err(SourceWordPendingReason::NoCompletedOtherIdentity)
        );
        let mut target = negative(at, Some(1000));
        target.classification = SessionSpeakerClassification::Target { score: 0.6 };
        assert_eq!(
            ledger.publish_other(target),
            Err(SourceWordPendingReason::NoCompletedOtherIdentity)
        );
    }

    #[test]
    fn immutable_native_receipts_never_renew_old_word_evidence_time() {
        let at = Instant::now();
        let later = at + Duration::from_secs(1);
        let repeated = later + Duration::from_secs(1);
        let mut ledger = CapturedWordAuthority::new(SOURCE);
        ledger
            .observe_native(
                SOURCE,
                binding(),
                &[span(1, 100, 900)],
                1000 * 16,
                1100 * 16,
                at,
            )
            .unwrap();
        let first = TimedTranscript {
            text: "本人".into(),
            words: vec![TimedTranscriptWord {
                text: "本人".into(),
                start_ms: Some(300),
                end_ms: Some(700),
            }],
        };
        assert_eq!(
            ledger
                .project(SOURCE, &first, false, 1100 * 16, at)
                .updated_at,
            Some(at)
        );
        let core = [span(1, 100, 900), span(1, 2300, 2900)];
        ledger
            .observe_native(SOURCE, binding(), &core, 3000 * 16, 3200 * 16, later)
            .unwrap();
        ledger
            .observe_native(SOURCE, binding(), &core, 3000 * 16, 3300 * 16, repeated)
            .unwrap();
        assert_eq!(
            ledger
                .project(SOURCE, &first, false, 3300 * 16, repeated)
                .updated_at,
            Some(at)
        );
        let mut other = negative(repeated, Some(1000));
        other.observed_at_input_sample = 3300 * 16;
        ledger.publish_other(other).unwrap();
        // Input-only growth updated the source clock, not the old permission.
        assert_eq!(
            ledger.identity(SOURCE, 1100, 1400, 3300 * 16, repeated),
            WordIdentity::Other
        );
    }

    #[test]
    fn stale_capture_changed_binding_and_rewritten_history_never_update_the_source() {
        let at = Instant::now();
        let later = at + Duration::from_secs(1);
        let mut ledger = authority(at);
        let core = [span(1, 100, 900), span(1, 2300, 2900)];
        let stale = SourceGeneration {
            capture_id: 4,
            ..SOURCE
        };
        assert_eq!(
            ledger.observe_native(stale, binding(), &core, 3000 * 16, 3200 * 16, later),
            Err(SourceWordPendingReason::SourceMismatch)
        );
        let changed = SourceWordBinding::from_qualified_channel(
            SOURCE,
            OwnerChannelBinding::Bound { channel: 2 },
            0,
            800 * 16,
        )
        .unwrap();
        assert_eq!(
            ledger.observe_native(SOURCE, changed, &core, 3000 * 16, 3200 * 16, later),
            Err(SourceWordPendingReason::BindingChanged)
        );
        let rewritten = [span(1, 101, 900), span(1, 2300, 2900)];
        assert_eq!(
            ledger.observe_native(SOURCE, binding(), &rewritten, 3000 * 16, 3200 * 16, later),
            Err(SourceWordPendingReason::RewrittenSettledHistory)
        );
        assert_eq!(
            ledger.identity(stale, 300, 700, 3200 * 16, later),
            WordIdentity::Pending
        );
        assert_eq!(
            ledger.identity(SOURCE, 300, 700, 3200 * 16, at - Duration::from_millis(1)),
            WordIdentity::Pending
        );
        assert_eq!(
            ledger.identity(SOURCE, 300, 700, 3200 * 16, later),
            WordIdentity::Owner
        );
        ledger.mark_unavailable();
        assert_eq!(
            ledger.identity(SOURCE, 300, 700, 3200 * 16, later),
            WordIdentity::Pending
        );
    }

    #[test]
    fn fractional_body_origin_needs_the_whole_word_settled_without_rounding() {
        let at = Instant::now();
        let later = at + Duration::from_secs(1);
        let binding = SourceWordBinding::from_verified_wake(VerifiedWakeOwnerSampleBinding {
            source: SOURCE,
            channel: 1,
            body_origin_sample: 17,
            valid_after_input_sample: 32,
        });
        let core = [AnonymousSampleSpan {
            channel: 1,
            range: SampleRange { start: 34, end: 48 },
        }];
        let mut ledger = CapturedWordAuthority::new(SOURCE);
        ledger
            .observe_native(SOURCE, binding, &core, 48, 64, at)
            .unwrap();
        assert_eq!(ledger.identity(SOURCE, 1, 2, 64, at), WordIdentity::Pending);
        ledger
            .observe_native(SOURCE, binding, &core, 49, 80, later)
            .unwrap();
        assert_eq!(
            ledger.identity(SOURCE, 1, 2, 80, later),
            WordIdentity::Owner
        );
        assert_eq!(ledger.identity(SOURCE, 1, 2, 80, at), WordIdentity::Pending);
        assert_eq!(
            ledger.identity(SOURCE, u64::MAX / 16, u64::MAX / 16 + 1, u64::MAX, later),
            WordIdentity::Pending
        );
    }

    #[test]
    fn other_source_cannot_erase_an_owner_bearing_boundary_or_lend_quality() {
        let at = Instant::now();
        let mut ledger = authority(at);
        let mut overlap = negative(at, Some(1000));
        overlap.range = SampleRange {
            start: 800 * 16,
            end: 2200 * 16,
        };
        assert_eq!(
            ledger.publish_other(overlap),
            Err(SourceWordPendingReason::OwnerConflict)
        );
        ledger.publish_other(negative(at, Some(1000))).unwrap();
        assert_eq!(
            ledger.identity(SOURCE, 850, 1100, 3200 * 16, at),
            WordIdentity::Pending
        );
        let mut invalid = negative(at, Some(1201));
        assert_eq!(
            ledger.publish_other(invalid),
            Err(SourceWordPendingReason::InvalidEvidence)
        );
        invalid = negative(at, Some(1000));
        invalid.vad_resolved_through_sample = 2199 * 16;
        assert_eq!(
            ledger.publish_other(invalid),
            Err(SourceWordPendingReason::FutureEvidence)
        );
        assert!(SourceWordBinding::from_qualified_channel(
            SOURCE,
            OwnerChannelBinding::Pending,
            0,
            100
        )
        .is_err());
    }
}
