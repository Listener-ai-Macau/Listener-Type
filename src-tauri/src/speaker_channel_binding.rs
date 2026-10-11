//! Session-local binding for anonymous diarization channels.
//!
//! This module does not score audio and contains no identity or VAD threshold.
//! Callers supply finalized `QualifiedTarget` / `QualifiedNonTarget` evidence
//! after applying the existing protected-profile and signal-quality contract.
//! Missing, delayed, or uncertain evidence remains pending.

use std::collections::{BTreeMap, BTreeSet};

pub(crate) const EXCLUSIVE_IDENTITY_WINDOW_MAX_MS: u64 = 3_000;
pub(crate) const EXCLUSIVE_IDENTITY_WINDOW_MIN_MS: u64 = 300;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct AnonymousSpan {
    pub(crate) channel: u8,
    pub(crate) start_ms: u64,
    pub(crate) end_ms: u64,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct ExclusiveIdentityWindow {
    pub(crate) channel: u8,
    pub(crate) start_ms: u64,
    pub(crate) end_ms: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum QualifiedIdentityEvidence {
    Pending,
    QualifiedTarget,
    QualifiedNonTarget,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum OwnerChannelBinding {
    Pending,
    Bound { channel: u8 },
    Ambiguous { qualified_channels: Vec<u8> },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ObservationDisposition {
    Accepted,
    Duplicate,
    FutureWindow,
    StaleGeneration,
    InactiveSource,
    UnplannedWindow,
}

/// Opaque source identity supplied by the capture coordinator.
///
/// `source_id` distinguishes logical audio streams, `capture_id` distinguishes
/// physical captures, and `generation` prevents a late proof from crossing a
/// decoder/source reset.  This module deliberately does not derive these IDs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SourceGeneration {
    pub(crate) source_id: u64,
    pub(crate) capture_id: u64,
    pub(crate) generation: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct MillisecondRange {
    pub(crate) start_ms: u64,
    pub(crate) end_ms: u64,
}

impl MillisecondRange {
    fn is_valid(self) -> bool {
        self.start_ms < self.end_ms
    }
}

/// Exact coordinates in a 16 kHz PCM clock. Original capture coordinates may
/// begin at any sample; none of them are rounded into milliseconds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SampleRange {
    pub(crate) start: u64,
    pub(crate) end: u64,
}

impl SampleRange {
    fn is_valid(self) -> bool {
        self.start < self.end
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AnonymousSampleSpan {
    pub(crate) channel: u8,
    pub(crate) range: SampleRange,
}

/// Only a positive decision from the already-enrolled protected wake profile
/// can bootstrap an anonymous native channel.  Open modes remain useful for
/// wake UX but carry no Owner identity authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WakeOwnerPolicy {
    EnrolledOwner,
    OpenUnenrolled,
    OpenInactiveProfile,
    EnrolledNonMatch,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct VerifiedWakeOwnerProof {
    pub(crate) source: SourceGeneration,
    pub(crate) policy: WakeOwnerPolicy,
    /// Range of the complete verifier input in the original capture clock.
    pub(crate) capture_range: MillisecondRange,
    /// Digest of the complete raw PCM verifier input, not a semantic wake crop.
    pub(crate) pcm_sha256: [u8; 32],
    /// Continuous-source input frontier at which the enrolled-owner result
    /// became observable. This is the same clock used by `continuous_range`,
    /// native frontiers, `body_origin_ms`, and the binding decision.
    pub(crate) observed_at_input_ms: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RetainedVerifiedWakeAudio {
    pub(crate) source: SourceGeneration,
    /// The exact original-capture bytes retained in the continuous source.
    pub(crate) capture_range: MillisecondRange,
    /// Their coordinates in the continuous diarization input.
    pub(crate) continuous_range: MillisecondRange,
    pub(crate) pcm_sha256: [u8; 32],
    /// Must be exactly one: repetition is not enrollment or identity proof.
    pub(crate) occurrence_count: u8,
    /// Set only by byte-continuity validation across the retained source.
    pub(crate) source_gap_free: bool,
    /// Origin of body-local timestamps in the continuous input clock.
    pub(crate) body_origin_ms: u64,
}

pub(crate) struct NativeWakeCoreEvidence<'a> {
    pub(crate) source: SourceGeneration,
    pub(crate) spans: &'a [AnonymousSpan],
    /// Irrevocable raw-frame frontier, never a provisional/display frontier.
    pub(crate) settled_through_ms: u64,
    /// Input frontier when this snapshot became observable.
    pub(crate) observed_at_input_ms: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct VerifiedWakeOwnerBinding {
    pub(crate) source: SourceGeneration,
    pub(crate) channel: u8,
    pub(crate) body_origin_ms: u64,
    /// The binding must not be applied to decisions before this frontier.
    pub(crate) valid_after_input_ms: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct VerifiedWakeOwnerSampleProof {
    pub(crate) source: SourceGeneration,
    pub(crate) policy: WakeOwnerPolicy,
    pub(crate) capture_range: SampleRange,
    pub(crate) pcm_sha256: [u8; 32],
    pub(crate) observed_at_input_sample: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RetainedVerifiedWakeSampleAudio {
    pub(crate) source: SourceGeneration,
    pub(crate) capture_range: SampleRange,
    pub(crate) continuous_range: SampleRange,
    pub(crate) pcm_sha256: [u8; 32],
    pub(crate) occurrence_count: u8,
    pub(crate) source_gap_free: bool,
    pub(crate) body_origin_sample: u64,
}

pub(crate) struct NativeWakeCoreSampleEvidence<'a> {
    pub(crate) source: SourceGeneration,
    pub(crate) spans: &'a [AnonymousSampleSpan],
    pub(crate) settled_through_sample: u64,
    pub(crate) observed_at_input_sample: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct VerifiedWakeOwnerSampleBinding {
    pub(crate) source: SourceGeneration,
    pub(crate) channel: u8,
    pub(crate) body_origin_sample: u64,
    pub(crate) valid_after_input_sample: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum VerifiedWakePendingReason {
    SourceMismatch,
    CaptureMismatch,
    StaleGeneration,
    UnqualifiedWakePolicy,
    InvalidRange,
    ProofMismatch,
    VerifiedAudioNotRetainedOnce,
    SourceGap,
    UnsettledNativeEvidence,
    FutureEvidence,
    MissingNativeCore,
    CompetingNativeCore,
    CoordinateOverflow,
}

fn same_source_generation(
    expected: SourceGeneration,
    actual: SourceGeneration,
) -> Result<(), VerifiedWakePendingReason> {
    if expected.source_id != actual.source_id {
        Err(VerifiedWakePendingReason::SourceMismatch)
    } else if expected.capture_id != actual.capture_id {
        Err(VerifiedWakePendingReason::CaptureMismatch)
    } else if expected.generation != actual.generation {
        Err(VerifiedWakePendingReason::StaleGeneration)
    } else {
        Ok(())
    }
}

/// Bind one anonymous channel from a retained, already-enrolled wake proof.
///
/// This consumes only coordinate/provenance facts and native core intervals;
/// it performs no speaker scoring and introduces no threshold.  The verifier's
/// complete raw input must occur exactly once in the same continuous source.
/// Native output must be settled across that full input interval. Exactly one
/// channel must have core activity somewhere in the interval; core activity is
/// not required to cover every millisecond. Any competing channel leaves
/// identity Pending instead of guessing which channel spoke the wake.
pub(crate) fn bind_verified_wake_owner_channel(
    proof: VerifiedWakeOwnerProof,
    retained: RetainedVerifiedWakeAudio,
    native: NativeWakeCoreEvidence<'_>,
    decision_input_ms: u64,
) -> Result<VerifiedWakeOwnerBinding, VerifiedWakePendingReason> {
    // Legacy whole-ms callers use the same source/identity decision as exact
    // PCM callers. Scaling is checked; no second, weaker binding policy exists.
    let spans = native
        .spans
        .iter()
        .map(|span| {
            Ok(AnonymousSampleSpan {
                channel: span.channel,
                range: ms_range_samples(MillisecondRange {
                    start_ms: span.start_ms,
                    end_ms: span.end_ms,
                })?,
            })
        })
        .collect::<Result<Vec<_>, VerifiedWakePendingReason>>()?;
    let bound = bind_verified_wake_owner_channel_samples(
        VerifiedWakeOwnerSampleProof {
            source: proof.source,
            policy: proof.policy,
            capture_range: ms_range_samples(proof.capture_range)?,
            pcm_sha256: proof.pcm_sha256,
            observed_at_input_sample: ms_samples(proof.observed_at_input_ms)?,
        },
        RetainedVerifiedWakeSampleAudio {
            source: retained.source,
            capture_range: ms_range_samples(retained.capture_range)?,
            continuous_range: ms_range_samples(retained.continuous_range)?,
            pcm_sha256: retained.pcm_sha256,
            occurrence_count: retained.occurrence_count,
            source_gap_free: retained.source_gap_free,
            body_origin_sample: ms_samples(retained.body_origin_ms)?,
        },
        NativeWakeCoreSampleEvidence {
            source: native.source,
            spans: &spans,
            settled_through_sample: ms_samples(native.settled_through_ms)?,
            observed_at_input_sample: ms_samples(native.observed_at_input_ms)?,
        },
        ms_samples(decision_input_ms)?,
    )?;
    // Every input above was an exact multiple of16, including the maximum
    // determining valid_after. Division here loses no information.
    Ok(VerifiedWakeOwnerBinding {
        source: bound.source,
        channel: bound.channel,
        body_origin_ms: bound.body_origin_sample / 16,
        valid_after_input_ms: bound.valid_after_input_sample / 16,
    })
}

/// The common verified-wake binding decision, in exact 16 kHz source samples.
/// Native core evidence must use this same sample clock and an audited settled
/// frontier. Single-sample conflicts and unfinished verifier tails are Pending.
pub(crate) fn bind_verified_wake_owner_channel_samples(
    proof: VerifiedWakeOwnerSampleProof,
    retained: RetainedVerifiedWakeSampleAudio,
    native: NativeWakeCoreSampleEvidence<'_>,
    decision_input_sample: u64,
) -> Result<VerifiedWakeOwnerSampleBinding, VerifiedWakePendingReason> {
    same_source_generation(proof.source, retained.source)?;
    same_source_generation(proof.source, native.source)?;
    if proof.policy != WakeOwnerPolicy::EnrolledOwner {
        return Err(VerifiedWakePendingReason::UnqualifiedWakePolicy);
    }
    if !proof.capture_range.is_valid()
        || !retained.capture_range.is_valid()
        || !retained.continuous_range.is_valid()
        || proof.capture_range != retained.capture_range
        || proof.capture_range.end - proof.capture_range.start
            != retained.continuous_range.end - retained.continuous_range.start
    {
        return Err(VerifiedWakePendingReason::InvalidRange);
    }
    if proof.pcm_sha256 != retained.pcm_sha256 {
        return Err(VerifiedWakePendingReason::ProofMismatch);
    }
    if retained.occurrence_count != 1 {
        return Err(VerifiedWakePendingReason::VerifiedAudioNotRetainedOnce);
    }
    if !retained.source_gap_free {
        return Err(VerifiedWakePendingReason::SourceGap);
    }
    if native.settled_through_sample < retained.continuous_range.end {
        return Err(VerifiedWakePendingReason::UnsettledNativeEvidence);
    }
    if native.settled_through_sample > native.observed_at_input_sample
        || native.observed_at_input_sample > decision_input_sample
        || proof.observed_at_input_sample > decision_input_sample
        || retained.body_origin_sample > decision_input_sample
    {
        return Err(VerifiedWakePendingReason::FutureEvidence);
    }
    let valid_after_input_sample = proof
        .observed_at_input_sample
        .max(native.observed_at_input_sample)
        .max(retained.continuous_range.end);
    if proof.observed_at_input_sample < retained.continuous_range.end
        || native.observed_at_input_sample < retained.continuous_range.end
        || valid_after_input_sample > decision_input_sample
    {
        return Err(VerifiedWakePendingReason::FutureEvidence);
    }

    let mut active_channels = BTreeSet::new();
    for span in native.spans {
        if !(1..=8).contains(&span.channel) || !span.range.is_valid() {
            return Err(VerifiedWakePendingReason::InvalidRange);
        }
        if span.range.start < retained.continuous_range.end
            && retained.continuous_range.start < span.range.end
        {
            active_channels.insert(span.channel);
        }
    }
    let channel = match active_channels
        .iter()
        .copied()
        .collect::<Vec<_>>()
        .as_slice()
    {
        [] => return Err(VerifiedWakePendingReason::MissingNativeCore),
        [channel] => *channel,
        _ => return Err(VerifiedWakePendingReason::CompetingNativeCore),
    };
    Ok(VerifiedWakeOwnerSampleBinding {
        source: proof.source,
        channel,
        body_origin_sample: retained.body_origin_sample,
        valid_after_input_sample,
    })
}

fn ms_samples(ms: u64) -> Result<u64, VerifiedWakePendingReason> {
    ms.checked_mul(16)
        .ok_or(VerifiedWakePendingReason::CoordinateOverflow)
}

fn ms_range_samples(range: MillisecondRange) -> Result<SampleRange, VerifiedWakePendingReason> {
    Ok(SampleRange {
        start: ms_samples(range.start_ms)?,
        end: ms_samples(range.end_ms)?,
    })
}

impl VerifiedWakeOwnerSampleBinding {
    /// Map the provider's body-local milliseconds without rounding either the
    /// true capture origin or the retained-wake observation frontier.
    pub(crate) fn map_body_range(
        self,
        source: SourceGeneration,
        body_range: MillisecondRange,
        available_input_sample: u64,
    ) -> Result<SampleRange, VerifiedWakePendingReason> {
        same_source_generation(self.source, source)?;
        if !body_range.is_valid() {
            return Err(VerifiedWakePendingReason::InvalidRange);
        }
        let body = ms_range_samples(body_range)?;
        let mapped = SampleRange {
            start: self
                .body_origin_sample
                .checked_add(body.start)
                .ok_or(VerifiedWakePendingReason::CoordinateOverflow)?,
            end: self
                .body_origin_sample
                .checked_add(body.end)
                .ok_or(VerifiedWakePendingReason::CoordinateOverflow)?,
        };
        if self.valid_after_input_sample > available_input_sample
            || mapped.end > available_input_sample
        {
            return Err(VerifiedWakePendingReason::FutureEvidence);
        }
        Ok(mapped)
    }
}

impl VerifiedWakeOwnerBinding {
    /// Map a body-local interval into the continuous source clock without
    /// allowing overflow, stale-source reuse, or evidence from the future.
    pub(crate) fn map_body_range(
        self,
        source: SourceGeneration,
        body_range: MillisecondRange,
        available_input_ms: u64,
    ) -> Result<MillisecondRange, VerifiedWakePendingReason> {
        same_source_generation(self.source, source)?;
        if !body_range.is_valid() {
            return Err(VerifiedWakePendingReason::InvalidRange);
        }
        let mapped = MillisecondRange {
            start_ms: self
                .body_origin_ms
                .checked_add(body_range.start_ms)
                .ok_or(VerifiedWakePendingReason::CoordinateOverflow)?,
            end_ms: self
                .body_origin_ms
                .checked_add(body_range.end_ms)
                .ok_or(VerifiedWakePendingReason::CoordinateOverflow)?,
        };
        if self.valid_after_input_ms > available_input_ms || mapped.end_ms > available_input_ms {
            return Err(VerifiedWakePendingReason::FutureEvidence);
        }
        Ok(mapped)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ChannelEvidence {
    has_qualified_target: bool,
    invalidated_by_qualified_non_target: bool,
}

/// Create the complete deterministic observation cadence from anonymous spans.
///
/// Same-channel spans are merged first. Activity from every other channel is
/// subtracted, so every returned window is exclusive to one anonymous channel.
/// Every exclusive span is tiled chronologically into immutable three-second
/// windows. A closed remainder of at least 300 ms is retained as its own
/// window. A remainder below 300 ms stays Pending; it is never backdated by
/// shrinking a previously published window. Online callers must supply only
/// causally settled spans and must not publish a short open tail until it has
/// either grown to 300 ms or closed.
pub(crate) fn plan_exclusive_identity_windows(
    spans: &[AnonymousSpan],
) -> Vec<ExclusiveIdentityWindow> {
    let mut by_channel: BTreeMap<u8, Vec<(u64, u64)>> = BTreeMap::new();
    for span in spans {
        if span.channel == 0 || span.start_ms >= span.end_ms {
            continue;
        }
        by_channel
            .entry(span.channel)
            .or_default()
            .push((span.start_ms, span.end_ms));
    }
    for intervals in by_channel.values_mut() {
        *intervals = merge_intervals(std::mem::take(intervals));
    }

    let mut windows = BTreeSet::new();
    for (&channel, own) in &by_channel {
        let other = merge_intervals(
            by_channel
                .iter()
                .filter(|(candidate, _)| **candidate != channel)
                .flat_map(|(_, intervals)| intervals.iter().copied())
                .collect(),
        );
        for &(start_ms, end_ms) in own {
            for (exclusive_start, exclusive_end) in subtract_intervals((start_ms, end_ms), &other) {
                for (window_start, window_end) in
                    tile_exclusive_span(exclusive_start, exclusive_end)
                {
                    windows.insert(ExclusiveIdentityWindow {
                        channel,
                        start_ms: window_start,
                        end_ms: window_end,
                    });
                }
            }
        }
    }

    let mut windows: Vec<_> = windows.into_iter().collect();
    windows.sort_by_key(|window| (window.start_ms, window.end_ms, window.channel));
    windows
}

fn merge_intervals(mut intervals: Vec<(u64, u64)>) -> Vec<(u64, u64)> {
    intervals.sort_unstable();
    let mut merged: Vec<(u64, u64)> = Vec::new();
    for (start, end) in intervals {
        if start >= end {
            continue;
        }
        if let Some(last) = merged.last_mut() {
            if start <= last.1 {
                last.1 = last.1.max(end);
                continue;
            }
        }
        merged.push((start, end));
    }
    merged
}

fn subtract_intervals(source: (u64, u64), exclusions: &[(u64, u64)]) -> Vec<(u64, u64)> {
    let mut result = Vec::new();
    let mut cursor = source.0;
    for &(start, end) in exclusions {
        if end <= cursor || start >= source.1 {
            continue;
        }
        if start > cursor {
            result.push((cursor, start.min(source.1)));
        }
        cursor = cursor.max(end);
        if cursor >= source.1 {
            break;
        }
    }
    if cursor < source.1 {
        result.push((cursor, source.1));
    }
    result
}

fn tile_exclusive_span(start_ms: u64, end_ms: u64) -> Vec<(u64, u64)> {
    let length = end_ms.saturating_sub(start_ms);
    if length < EXCLUSIVE_IDENTITY_WINDOW_MIN_MS {
        return Vec::new();
    }
    if length <= EXCLUSIVE_IDENTITY_WINDOW_MAX_MS {
        return vec![(start_ms, end_ms)];
    }

    let full_tiles = length / EXCLUSIVE_IDENTITY_WINDOW_MAX_MS;
    let remainder = length % EXCLUSIVE_IDENTITY_WINDOW_MAX_MS;
    let mut lengths = vec![EXCLUSIVE_IDENTITY_WINDOW_MAX_MS; full_tiles as usize];
    if remainder >= EXCLUSIVE_IDENTITY_WINDOW_MIN_MS {
        lengths.push(remainder);
    }

    let mut cursor = start_ms;
    lengths
        .into_iter()
        .map(|length| {
            let window = (cursor, cursor + length);
            cursor = window.1;
            window
        })
        .collect()
}

/// Accumulates only finalized, source-local observations for one generation.
///
/// A channel is bound after at least one qualified Target observation unless a
/// qualified NonTarget observation invalidates that channel. More than one
/// eligible channel is ambiguous. Each planned window contributes at most once.
pub(crate) struct OwnerChannelBinder {
    generation: u64,
    active: bool,
    planned: BTreeSet<ExclusiveIdentityWindow>,
    observed: BTreeSet<ExclusiveIdentityWindow>,
    channels: BTreeMap<u8, ChannelEvidence>,
}

impl OwnerChannelBinder {
    pub(crate) fn new(
        generation: u64,
        planned: impl IntoIterator<Item = ExclusiveIdentityWindow>,
    ) -> Self {
        Self {
            generation,
            active: true,
            planned: planned.into_iter().collect(),
            observed: BTreeSet::new(),
            channels: BTreeMap::new(),
        }
    }

    pub(crate) fn begin_source(
        &mut self,
        generation: u64,
        planned: impl IntoIterator<Item = ExclusiveIdentityWindow>,
    ) -> bool {
        if generation == self.generation {
            return false;
        }
        self.generation = generation;
        self.active = true;
        self.planned = planned.into_iter().collect();
        self.observed.clear();
        self.channels.clear();
        true
    }

    pub(crate) fn invalidate_source(&mut self, generation: u64) -> bool {
        if generation != self.generation || !self.active {
            return false;
        }
        self.active = false;
        self.planned.clear();
        self.observed.clear();
        self.channels.clear();
        true
    }

    pub(crate) fn observe(
        &mut self,
        generation: u64,
        window: ExclusiveIdentityWindow,
        observed_through_ms: u64,
        evidence: QualifiedIdentityEvidence,
    ) -> ObservationDisposition {
        if generation != self.generation {
            return ObservationDisposition::StaleGeneration;
        }
        if !self.active {
            return ObservationDisposition::InactiveSource;
        }
        if !self.planned.contains(&window) {
            return ObservationDisposition::UnplannedWindow;
        }
        if window.end_ms > observed_through_ms {
            return ObservationDisposition::FutureWindow;
        }
        if !self.observed.insert(window) {
            return ObservationDisposition::Duplicate;
        }

        let channel = self.channels.entry(window.channel).or_default();
        match evidence {
            QualifiedIdentityEvidence::Pending => {}
            QualifiedIdentityEvidence::QualifiedTarget => {
                channel.has_qualified_target = true;
            }
            QualifiedIdentityEvidence::QualifiedNonTarget => {
                channel.invalidated_by_qualified_non_target = true;
            }
        }
        ObservationDisposition::Accepted
    }

    pub(crate) fn binding(&self) -> OwnerChannelBinding {
        if !self.active {
            return OwnerChannelBinding::Pending;
        }
        let qualified_channels: Vec<_> = self
            .channels
            .iter()
            .filter_map(|(&channel, evidence)| {
                (evidence.has_qualified_target && !evidence.invalidated_by_qualified_non_target)
                    .then_some(channel)
            })
            .collect();
        match qualified_channels.as_slice() {
            [] => OwnerChannelBinding::Pending,
            [channel] => OwnerChannelBinding::Bound { channel: *channel },
            _ => OwnerChannelBinding::Ambiguous { qualified_channels },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VERIFIED_AUDIO_DIGEST: [u8; 32] = [0x99; 32];

    fn source() -> SourceGeneration {
        SourceGeneration {
            source_id: 139,
            capture_id: 715_587_594,
            generation: 7,
        }
    }

    fn verified_wake_proof() -> VerifiedWakeOwnerProof {
        VerifiedWakeOwnerProof {
            source: source(),
            policy: WakeOwnerPolicy::EnrolledOwner,
            capture_range: MillisecondRange {
                start_ms: 0,
                end_ms: 1_836,
            },
            pcm_sha256: VERIFIED_AUDIO_DIGEST,
            observed_at_input_ms: 1_887,
        }
    }

    fn retained_wake() -> RetainedVerifiedWakeAudio {
        RetainedVerifiedWakeAudio {
            source: source(),
            capture_range: MillisecondRange {
                start_ms: 0,
                end_ms: 1_836,
            },
            continuous_range: MillisecondRange {
                start_ms: 0,
                end_ms: 1_836,
            },
            pcm_sha256: VERIFIED_AUDIO_DIGEST,
            occurrence_count: 1,
            source_gap_free: true,
            body_origin_ms: 916,
        }
    }

    fn recorded_core() -> Vec<AnonymousSpan> {
        vec![
            AnonymousSpan {
                channel: 1,
                start_ms: 1_260,
                end_ms: 5_500,
            },
            AnonymousSpan {
                channel: 1,
                start_ms: 5_940,
                end_ms: 8_710,
            },
        ]
    }

    fn native<'a>(spans: &'a [AnonymousSpan]) -> NativeWakeCoreEvidence<'a> {
        NativeWakeCoreEvidence {
            source: source(),
            spans,
            settled_through_ms: 2_080,
            observed_at_input_ms: 2_240,
        }
    }

    fn window(channel: u8, start_ms: u64, end_ms: u64) -> ExclusiveIdentityWindow {
        ExclusiveIdentityWindow {
            channel,
            start_ms,
            end_ms,
        }
    }

    #[test]
    fn planner_tiles_every_exclusive_span_chronologically_without_duplicates() {
        let planned = plan_exclusive_identity_windows(&[
            AnonymousSpan {
                channel: 1,
                start_ms: 0,
                end_ms: 6_500,
            },
            AnonymousSpan {
                channel: 1,
                start_ms: 0,
                end_ms: 6_500,
            },
            AnonymousSpan {
                channel: 2,
                start_ms: 1_000,
                end_ms: 1_600,
            },
            AnonymousSpan {
                channel: 3,
                start_ms: 7_000,
                end_ms: 10_200,
            },
            AnonymousSpan {
                channel: 4,
                start_ms: 10_500,
                end_ms: 10_799,
            },
        ]);
        assert_eq!(
            planned,
            vec![
                window(1, 0, 1_000),
                window(1, 1_600, 4_600),
                window(1, 4_600, 6_500),
                window(3, 7_000, 10_000),
            ]
        );
        assert!(planned.iter().all(|candidate| {
            let length = candidate.end_ms - candidate.start_ms;
            (EXCLUSIVE_IDENTITY_WINDOW_MIN_MS..=EXCLUSIVE_IDENTITY_WINDOW_MAX_MS).contains(&length)
        }));
    }

    #[test]
    fn growing_open_span_never_rewrites_a_previously_completed_tile() {
        let first = plan_exclusive_identity_windows(&[AnonymousSpan {
            channel: 1,
            start_ms: 0,
            end_ms: 3_200,
        }]);
        assert_eq!(first, vec![window(1, 0, 3_000)]);

        let grown = plan_exclusive_identity_windows(&[AnonymousSpan {
            channel: 1,
            start_ms: 0,
            end_ms: 6_100,
        }]);
        assert_eq!(grown, vec![window(1, 0, 3_000), window(1, 3_000, 6_000)]);
        assert_eq!(first[0], grown[0]);
    }

    #[test]
    fn later_target_binds_after_an_earlier_uncertain_window() {
        let first = window(1, 0, 3_000);
        let later = window(1, 3_000, 6_000);
        let mut binder = OwnerChannelBinder::new(7, [first, later]);
        assert_eq!(
            binder.observe(7, first, 3_000, QualifiedIdentityEvidence::Pending),
            ObservationDisposition::Accepted
        );
        assert_eq!(binder.binding(), OwnerChannelBinding::Pending);
        assert_eq!(
            binder.observe(7, later, 6_000, QualifiedIdentityEvidence::QualifiedTarget),
            ObservationDisposition::Accepted
        );
        assert_eq!(binder.binding(), OwnerChannelBinding::Bound { channel: 1 });
    }

    #[test]
    fn qualified_non_target_permanently_invalidates_that_channel_for_the_source() {
        let target = window(2, 0, 3_000);
        let conflict = window(2, 3_000, 6_000);
        let mut binder = OwnerChannelBinder::new(8, [target, conflict]);
        binder.observe(8, target, 3_000, QualifiedIdentityEvidence::QualifiedTarget);
        assert_eq!(binder.binding(), OwnerChannelBinding::Bound { channel: 2 });
        binder.observe(
            8,
            conflict,
            6_000,
            QualifiedIdentityEvidence::QualifiedNonTarget,
        );
        assert_eq!(binder.binding(), OwnerChannelBinding::Pending);
    }

    #[test]
    fn multiple_qualified_channels_are_ambiguous() {
        let one = window(1, 0, 1_000);
        let two = window(2, 1_000, 2_000);
        let mut binder = OwnerChannelBinder::new(9, [one, two]);
        binder.observe(9, one, 1_000, QualifiedIdentityEvidence::QualifiedTarget);
        binder.observe(9, two, 2_000, QualifiedIdentityEvidence::QualifiedTarget);
        assert_eq!(
            binder.binding(),
            OwnerChannelBinding::Ambiguous {
                qualified_channels: vec![1, 2]
            }
        );
    }

    #[test]
    fn repeated_window_cannot_flip_an_accepted_observation() {
        let only = window(1, 0, 1_000);
        let mut binder = OwnerChannelBinder::new(10, [only]);
        binder.observe(10, only, 1_000, QualifiedIdentityEvidence::QualifiedTarget);
        assert_eq!(
            binder.observe(
                10,
                only,
                1_000,
                QualifiedIdentityEvidence::QualifiedNonTarget,
            ),
            ObservationDisposition::Duplicate
        );
        assert_eq!(binder.binding(), OwnerChannelBinding::Bound { channel: 1 });
    }

    #[test]
    fn future_and_stale_evidence_cannot_cross_source_generations() {
        let old = window(1, 0, 1_000);
        let fresh = window(2, 0, 1_000);
        let mut binder = OwnerChannelBinder::new(11, [old]);
        assert_eq!(
            binder.observe(11, old, 999, QualifiedIdentityEvidence::QualifiedTarget),
            ObservationDisposition::FutureWindow
        );
        assert_eq!(binder.binding(), OwnerChannelBinding::Pending);
        assert!(!binder.begin_source(11, [fresh]));
        assert!(binder.begin_source(12, [fresh]));
        assert_eq!(
            binder.observe(11, old, 1_000, QualifiedIdentityEvidence::QualifiedTarget),
            ObservationDisposition::StaleGeneration
        );
        assert_eq!(
            binder.observe(12, fresh, 1_000, QualifiedIdentityEvidence::QualifiedTarget),
            ObservationDisposition::Accepted
        );
        assert_eq!(binder.binding(), OwnerChannelBinding::Bound { channel: 2 });
        assert!(binder.invalidate_source(12));
        assert_eq!(binder.binding(), OwnerChannelBinding::Pending);
        assert_eq!(
            binder.observe(12, fresh, 1_000, QualifiedIdentityEvidence::QualifiedTarget),
            ObservationDisposition::InactiveSource
        );
    }

    #[test]
    fn an_unplanned_window_cannot_inject_identity() {
        let planned = window(1, 0, 1_000);
        let invented = window(1, 1_000, 2_000);
        let mut binder = OwnerChannelBinder::new(13, [planned]);
        assert_eq!(
            binder.observe(
                13,
                invented,
                2_000,
                QualifiedIdentityEvidence::QualifiedTarget,
            ),
            ObservationDisposition::UnplannedWindow
        );
        assert_eq!(binder.binding(), OwnerChannelBinding::Pending);
    }

    #[test]
    fn exact_recorded_wake_binds_unique_channel_and_maps_all_39_body_words() {
        let core = recorded_core();
        let binding = bind_verified_wake_owner_channel(
            verified_wake_proof(),
            retained_wake(),
            native(&core),
            2_240,
        )
        .unwrap();
        assert_eq!(binding.channel, 1);
        assert_eq!(binding.body_origin_ms, 916);
        assert_eq!(binding.valid_after_input_ms, 2_240);

        // Actual final provider word ranges after the four wake characters.
        let body_words = [
            (1_120, 1_280),
            (1_280, 1_400),
            (1_400, 1_520),
            (1_520, 1_600),
            (1_600, 1_800),
            (1_800, 1_920),
            (1_920, 2_040),
            (2_040, 2_200),
            (2_200, 2_320),
            (2_360, 2_520),
            (2_520, 2_560),
            (2_600, 2_760),
            (2_840, 2_920),
            (3_120, 3_160),
            (3_480, 3_680),
            (3_680, 3_800),
            (3_840, 4_000),
            (4_000, 4_120),
            (4_120, 4_320),
            (4_320, 4_480),
            (5_112, 5_352),
            (5_352, 5_552),
            (5_552, 5_792),
            (5_792, 5_872),
            (5_872, 5_992),
            (6_072, 6_192),
            (6_232, 6_272),
            (6_392, 6_552),
            (6_552, 6_632),
            (6_632, 6_752),
            (6_752, 6_872),
            (6_872, 6_992),
            (6_992, 7_072),
            (7_072, 7_192),
            (7_192, 7_272),
            (7_272, 7_392),
            (7_392, 7_552),
            (7_552, 7_712),
            (7_712, 7_872),
        ];
        let mapped: Vec<_> = body_words
            .into_iter()
            .map(|(start_ms, end_ms)| {
                binding
                    .map_body_range(source(), MillisecondRange { start_ms, end_ms }, 16_160)
                    .unwrap()
            })
            .collect();
        assert_eq!(mapped.len(), 39);
        assert_eq!(
            mapped[0],
            MillisecondRange {
                start_ms: 2_036,
                end_ms: 2_196
            }
        );
        assert_eq!(
            mapped[38],
            MillisecondRange {
                start_ms: 8_628,
                end_ms: 8_788
            }
        );
        assert!(mapped.iter().all(|word| core.iter().any(|span| {
            span.channel == binding.channel
                && span.start_ms < word.end_ms
                && word.start_ms < span.end_ms
        })));
    }

    #[test]
    fn crop_only_body_cannot_borrow_the_complete_wake_proof() {
        let mut cropped = retained_wake();
        cropped.capture_range = MillisecondRange {
            start_ms: 916,
            end_ms: 1_836,
        };
        cropped.continuous_range = MillisecondRange {
            start_ms: 0,
            end_ms: 920,
        };
        assert_eq!(
            bind_verified_wake_owner_channel(
                verified_wake_proof(),
                cropped,
                native(&recorded_core()),
                2_240,
            ),
            Err(VerifiedWakePendingReason::InvalidRange)
        );
    }

    #[test]
    fn open_or_inactive_profiles_never_supply_owner_identity() {
        for policy in [
            WakeOwnerPolicy::OpenUnenrolled,
            WakeOwnerPolicy::OpenInactiveProfile,
            WakeOwnerPolicy::EnrolledNonMatch,
        ] {
            let mut proof = verified_wake_proof();
            proof.policy = policy;
            assert_eq!(
                bind_verified_wake_owner_channel(
                    proof,
                    retained_wake(),
                    native(&recorded_core()),
                    2_240,
                ),
                Err(VerifiedWakePendingReason::UnqualifiedWakePolicy)
            );
        }
    }

    #[test]
    fn competing_or_missing_native_core_remains_pending() {
        let mut competing = recorded_core();
        competing.push(AnonymousSpan {
            channel: 2,
            start_ms: 1_500,
            end_ms: 1_700,
        });
        assert_eq!(
            bind_verified_wake_owner_channel(
                verified_wake_proof(),
                retained_wake(),
                native(&competing),
                2_240,
            ),
            Err(VerifiedWakePendingReason::CompetingNativeCore)
        );
        assert_eq!(
            bind_verified_wake_owner_channel(
                verified_wake_proof(),
                retained_wake(),
                native(&[]),
                2_240,
            ),
            Err(VerifiedWakePendingReason::MissingNativeCore)
        );
    }

    #[test]
    fn stale_generation_future_or_unsettled_evidence_cannot_backdate_binding() {
        let core = recorded_core();
        let mut stale = retained_wake();
        stale.source.generation += 1;
        assert_eq!(
            bind_verified_wake_owner_channel(verified_wake_proof(), stale, native(&core), 2_240,),
            Err(VerifiedWakePendingReason::StaleGeneration)
        );

        let mut future = verified_wake_proof();
        future.observed_at_input_ms = 2_500;
        assert_eq!(
            bind_verified_wake_owner_channel(future, retained_wake(), native(&core), 2_240),
            Err(VerifiedWakePendingReason::FutureEvidence)
        );

        let unsettled = NativeWakeCoreEvidence {
            settled_through_ms: 1_835,
            ..native(&core)
        };
        assert_eq!(
            bind_verified_wake_owner_channel(
                verified_wake_proof(),
                retained_wake(),
                unsettled,
                2_240,
            ),
            Err(VerifiedWakePendingReason::UnsettledNativeEvidence)
        );

        let binding = bind_verified_wake_owner_channel(
            verified_wake_proof(),
            retained_wake(),
            native(&core),
            2_240,
        )
        .unwrap();
        assert_eq!(
            binding.map_body_range(
                source(),
                MillisecondRange {
                    start_ms: 1_120,
                    end_ms: 1_280
                },
                2_200,
            ),
            Err(VerifiedWakePendingReason::FutureEvidence)
        );
    }

    #[test]
    fn native_frontiers_and_body_origin_share_one_causal_source_clock() {
        let core = recorded_core();

        let settled_beyond_observation = NativeWakeCoreEvidence {
            settled_through_ms: 2_241,
            observed_at_input_ms: 2_240,
            ..native(&core)
        };
        assert_eq!(
            bind_verified_wake_owner_channel(
                verified_wake_proof(),
                retained_wake(),
                settled_beyond_observation,
                2_240,
            ),
            Err(VerifiedWakePendingReason::FutureEvidence)
        );

        let observed_after_decision = NativeWakeCoreEvidence {
            settled_through_ms: 2_080,
            observed_at_input_ms: 2_241,
            ..native(&core)
        };
        assert_eq!(
            bind_verified_wake_owner_channel(
                verified_wake_proof(),
                retained_wake(),
                observed_after_decision,
                2_240,
            ),
            Err(VerifiedWakePendingReason::FutureEvidence)
        );

        let mut future_body = retained_wake();
        future_body.body_origin_ms = 2_241;
        assert_eq!(
            bind_verified_wake_owner_channel(
                verified_wake_proof(),
                future_body,
                native(&core),
                2_240,
            ),
            Err(VerifiedWakePendingReason::FutureEvidence)
        );

        let adjacent = NativeWakeCoreEvidence {
            settled_through_ms: 2_240,
            observed_at_input_ms: 2_240,
            ..native(&core)
        };
        let mut adjacent_body = retained_wake();
        adjacent_body.body_origin_ms = 2_240;
        let binding =
            bind_verified_wake_owner_channel(verified_wake_proof(), adjacent_body, adjacent, 2_240)
                .unwrap();
        assert_eq!(binding.valid_after_input_ms, 2_240);
        assert_eq!(binding.body_origin_ms, 2_240);
    }

    #[test]
    fn source_gap_duplicate_or_proof_mismatch_stays_pending() {
        let core = recorded_core();
        let mut gap = retained_wake();
        gap.source_gap_free = false;
        assert_eq!(
            bind_verified_wake_owner_channel(verified_wake_proof(), gap, native(&core), 2_240,),
            Err(VerifiedWakePendingReason::SourceGap)
        );

        let mut repeated = retained_wake();
        repeated.occurrence_count = 2;
        assert_eq!(
            bind_verified_wake_owner_channel(verified_wake_proof(), repeated, native(&core), 2_240,),
            Err(VerifiedWakePendingReason::VerifiedAudioNotRetainedOnce)
        );

        let mut mismatch = retained_wake();
        mismatch.pcm_sha256 = [0x42; 32];
        assert_eq!(
            bind_verified_wake_owner_channel(verified_wake_proof(), mismatch, native(&core), 2_240,),
            Err(VerifiedWakePendingReason::ProofMismatch)
        );
    }

    #[test]
    fn checked_body_clock_mapping_rejects_overflow_and_wrong_source() {
        let core = recorded_core();
        let binding = bind_verified_wake_owner_channel(
            verified_wake_proof(),
            retained_wake(),
            native(&core),
            2_240,
        )
        .unwrap();
        let overflow = VerifiedWakeOwnerBinding {
            body_origin_ms: u64::MAX - 10,
            ..binding
        };
        assert_eq!(
            overflow.map_body_range(
                source(),
                MillisecondRange {
                    start_ms: 20,
                    end_ms: 30
                },
                u64::MAX,
            ),
            Err(VerifiedWakePendingReason::CoordinateOverflow)
        );
        let mut wrong = source();
        wrong.capture_id += 1;
        assert_eq!(
            binding.map_body_range(
                wrong,
                MillisecondRange {
                    start_ms: 0,
                    end_ms: 100
                },
                2_240,
            ),
            Err(VerifiedWakePendingReason::CaptureMismatch)
        );
    }

    fn exact_sample_proof() -> VerifiedWakeOwnerSampleProof {
        VerifiedWakeOwnerSampleProof {
            source: source(),
            policy: WakeOwnerPolicy::EnrolledOwner,
            capture_range: SampleRange {
                start: 403,
                end: 29_684,
            },
            pcm_sha256: VERIFIED_AUDIO_DIGEST,
            observed_at_input_sample: 30_193,
        }
    }

    fn exact_sample_retention() -> RetainedVerifiedWakeSampleAudio {
        RetainedVerifiedWakeSampleAudio {
            source: source(),
            capture_range: exact_sample_proof().capture_range,
            continuous_range: SampleRange {
                start: 0,
                end: 29_281,
            },
            pcm_sha256: VERIFIED_AUDIO_DIGEST,
            occurrence_count: 1,
            source_gap_free: true,
            body_origin_sample: 14_657,
        }
    }

    fn exact_sample_native(spans: &[AnonymousSampleSpan]) -> NativeWakeCoreSampleEvidence<'_> {
        NativeWakeCoreSampleEvidence {
            source: source(),
            spans,
            settled_through_sample: 33_280,
            observed_at_input_sample: 35_313,
        }
    }

    #[test]
    fn fractional_capture_and_body_origin_keep_the_last_verifier_sample_pending() {
        let core = [AnonymousSampleSpan {
            channel: 1,
            range: SampleRange {
                start: 20_160,
                end: 33_280,
            },
        }];
        let unsettled = NativeWakeCoreSampleEvidence {
            settled_through_sample: 29_280,
            ..exact_sample_native(&core)
        };
        assert_eq!(
            bind_verified_wake_owner_channel_samples(
                exact_sample_proof(),
                exact_sample_retention(),
                unsettled,
                35_313
            ),
            Err(VerifiedWakePendingReason::UnsettledNativeEvidence)
        );
        let bound = bind_verified_wake_owner_channel_samples(
            exact_sample_proof(),
            exact_sample_retention(),
            exact_sample_native(&core),
            35_313,
        )
        .unwrap();
        assert_eq!(bound.channel, 1);
        assert_eq!(bound.body_origin_sample, 14_657);
        assert_eq!(bound.valid_after_input_sample, 35_313);
        let word = MillisecondRange {
            start_ms: 1_120,
            end_ms: 1_280,
        };
        assert_eq!(
            bound.map_body_range(source(), word, 35_313).unwrap(),
            SampleRange {
                start: 32_577,
                end: 35_137
            }
        );
        assert_eq!(
            bound.map_body_range(source(), word, 35_312),
            Err(VerifiedWakePendingReason::FutureEvidence)
        );
    }

    #[test]
    fn one_competing_sample_at_the_verifier_edge_prevents_binding() {
        let mut core = vec![AnonymousSampleSpan {
            channel: 1,
            range: SampleRange {
                start: 20_160,
                end: 33_280,
            },
        }];
        core.push(AnonymousSampleSpan {
            channel: 2,
            range: SampleRange {
                start: 29_280,
                end: 29_440,
            },
        });
        assert_eq!(
            bind_verified_wake_owner_channel_samples(
                exact_sample_proof(),
                exact_sample_retention(),
                exact_sample_native(&core),
                35_313
            ),
            Err(VerifiedWakePendingReason::CompetingNativeCore)
        );
        // Half-open intervals that touch the exact edge do not overlap it.
        core[1].range.start = 29_281;
        assert_eq!(
            bind_verified_wake_owner_channel_samples(
                exact_sample_proof(),
                exact_sample_retention(),
                exact_sample_native(&core),
                35_313
            )
            .unwrap()
            .channel,
            1
        );
    }

    #[test]
    fn exact_sample_binding_rejects_future_source_and_coordinate_overflow() {
        let core = [AnonymousSampleSpan {
            channel: 1,
            range: SampleRange {
                start: 20_160,
                end: 33_280,
            },
        }];
        assert_eq!(
            bind_verified_wake_owner_channel_samples(
                exact_sample_proof(),
                exact_sample_retention(),
                exact_sample_native(&core),
                35_312
            ),
            Err(VerifiedWakePendingReason::FutureEvidence)
        );
        let mut stale = exact_sample_retention();
        stale.source.generation += 1;
        assert_eq!(
            bind_verified_wake_owner_channel_samples(
                exact_sample_proof(),
                stale,
                exact_sample_native(&core),
                35_313
            ),
            Err(VerifiedWakePendingReason::StaleGeneration)
        );
        let bound = bind_verified_wake_owner_channel_samples(
            exact_sample_proof(),
            exact_sample_retention(),
            exact_sample_native(&core),
            35_313,
        )
        .unwrap();
        let word = MillisecondRange {
            start_ms: u64::MAX / 16,
            end_ms: u64::MAX / 16 + 1,
        };
        assert_eq!(
            bound.map_body_range(source(), word, u64::MAX),
            Err(VerifiedWakePendingReason::CoordinateOverflow)
        );
        let overflowing_origin = VerifiedWakeOwnerSampleBinding {
            body_origin_sample: u64::MAX - 1,
            ..bound
        };
        assert_eq!(
            overflowing_origin.map_body_range(
                source(),
                MillisecondRange {
                    start_ms: 0,
                    end_ms: 1
                },
                u64::MAX
            ),
            Err(VerifiedWakePendingReason::CoordinateOverflow)
        );
    }
}
