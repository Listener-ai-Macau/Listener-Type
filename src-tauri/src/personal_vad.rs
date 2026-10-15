//! Speaker-conditioned activity for endpointing. Presence never certifies words.

use std::time::{Duration, Instant};

/// Exact capture coordinates, independent of message size and worker speed.
/// A worker consumes every sample, but runs the model once for all complete
/// chunks already waiting. The model window owns the natural batch bound.
#[cfg(any(target_os = "windows", test))]
#[derive(Default)]
pub(super) struct PresenceAudioBuffer {
    pending: Vec<u8>,
    history: Vec<u8>,
    origin_samples: Option<u64>,
    accepted_samples: u64,
    analyzed_samples: u64,
}

#[cfg(any(target_os = "windows", test))]
pub(super) struct PresenceAudioWindow {
    pub pcm: Vec<u8>,
    pub new_chunks: usize,
    pub first_new_end_samples: u64,
}

#[cfg(any(target_os = "windows", test))]
impl PresenceAudioBuffer {
    pub(super) const CHUNK_SAMPLES: usize = 2_560;
    const WINDOW_CHUNKS: usize = 15;

    pub(super) fn push(&mut self, pcm: &[u8], source_end_samples: u64) -> Result<(), String> {
        if pcm.len() % 2 != 0 { return Err("personal VAD received an incomplete PCM16 sample".into()); }
        let samples = (pcm.len() / 2) as u64;
        if samples == 0 { return Ok(()); }
        let origin = match self.origin_samples {
            Some(origin) => origin,
            None => source_end_samples.checked_sub(samples)
                .ok_or("personal VAD source end precedes its audio")?,
        };
        let accepted = self.accepted_samples.checked_add(samples)
            .ok_or("personal VAD source timeline overflowed")?;
        if origin.checked_add(accepted) != Some(source_end_samples) {
            return Err("personal VAD source timeline is discontinuous".into());
        }
        self.origin_samples = Some(origin);
        self.accepted_samples = accepted;
        self.pending.extend_from_slice(pcm);
        Ok(())
    }

    pub(super) fn has_complete_chunk(&self) -> bool {
        self.pending.len() >= Self::CHUNK_SAMPLES * 2
    }

    pub(super) fn origin_ms(&self) -> u64 { self.origin_samples.unwrap_or_default() / 16 }

    pub(super) fn next_window(&mut self) -> Option<PresenceAudioWindow> {
        let chunks = (self.pending.len() / (Self::CHUNK_SAMPLES * 2)).min(Self::WINDOW_CHUNKS);
        if chunks == 0 { return None; }
        let first_new_end_samples = self.origin_samples?
            .checked_add(self.analyzed_samples)?
            .checked_add(Self::CHUNK_SAMPLES as u64)?;
        let bytes = chunks * Self::CHUNK_SAMPLES * 2;
        self.history.extend(self.pending.drain(..bytes));
        let history_limit = Self::WINDOW_CHUNKS * Self::CHUNK_SAMPLES * 2;
        if self.history.len() > history_limit {
            self.history.drain(..self.history.len() - history_limit);
        }
        self.analyzed_samples += (bytes / 2) as u64;
        Some(PresenceAudioWindow { pcm: self.history.clone(), new_chunks: chunks, first_new_end_samples })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PresenceState {
    Owner,
    ConfirmedOther,
    Unknown,
}

/// Word projection also requires native speaker separation. Recording lifetime
/// has no such word-level veto, so it must retain its stronger identity tier.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PresenceIdentityObservation {
    pub projection: PresenceState,
    pub endpoint: PresenceState,
}

pub(crate) type PresenceVerifier = Box<dyn FnMut(&[u8]) -> Result<PresenceIdentityObservation, String> + Send>;

/// This type can publish only the verifier's endpoint grade. Ordinary word
/// projection's weaker Other cannot be passed as an owner-absence certificate.
#[cfg(any(target_os = "windows", test))]
#[derive(Clone, Debug, Default)]
pub(crate) struct EndpointSourceTimeline {
    evidence: crate::asr::volcengine_speaker_ledger::LocalSpeakerTimeline,
}

#[cfg(any(target_os = "windows", test))]
impl EndpointSourceTimeline {
    pub(super) fn note_scored_window(&mut self, scores: &[f32], first_new_end_samples: u64,
        new_chunks: usize, identity: Option<PresenceIdentityObservation>) -> Result<(), String> {
        publish_scored_window(&mut self.evidence, scores, first_new_end_samples, new_chunks,
            identity.map(|identity| identity.endpoint))
    }

    pub(crate) fn confirms_other(&self, start: u64, end: u64) -> bool {
        self.evidence.confirms_other(start, end)
    }

    pub(super) fn unresolved_after(&self, owner_end: u64) -> Option<u64> {
        self.evidence.unresolved_activity_end_ms_after(owner_end)
    }
}

pub(super) const OWNER_ACTIVITY_MIN_PROBABILITY: f32 = 0.7;

/// A rolling-window positive belongs to its original source chunk, even when
/// that chunk is context for newly captured audio. It never advances to now.
#[cfg(any(target_os = "windows", test))]
pub(super) fn latest_target_source_end(
    scores: &[f32], first_new_end_samples: u64, new_chunks: usize,
) -> Option<u64> {
    if new_chunks == 0 || new_chunks > scores.len() { return None; }
    let context_samples = ((scores.len()-new_chunks) * PresenceAudioBuffer::CHUNK_SAMPLES) as u64;
    let first_end = first_new_end_samples.checked_sub(context_samples)?;
    let index = scores.iter().rposition(|score| *score >= OWNER_ACTIVITY_MIN_PROBABILITY)?;
    first_end.checked_add((index * PresenceAudioBuffer::CHUNK_SAMPLES) as u64)
}

/// A target-only model's rejection is not evidence that the owner is absent.
/// Independent, quality-qualified identity may rescue the owner or confirm
/// another speaker. Conflicting positive evidence always preserves recording.
pub(super) fn fuse_presence(probability: f32, identity: Option<PresenceState>) -> PresenceState {
    if probability >= OWNER_ACTIVITY_MIN_PROBABILITY || identity == Some(PresenceState::Owner) {
        PresenceState::Owner
    } else if identity == Some(PresenceState::ConfirmedOther) {
        PresenceState::ConfirmedOther
    } else {
        PresenceState::Unknown
    }
}

/// Publish every qualified update at the model window's immutable source
/// coordinates, including Other evidence for previously pending context.
/// Activity rejection alone stays Unknown; original Owner positives are sticky.
#[cfg(any(target_os = "windows", test))]
pub(crate) fn publish_scored_window(
    timeline: &mut crate::asr::volcengine_speaker_ledger::LocalSpeakerTimeline,
    scores: &[f32], first_new_end_samples: u64, new_chunks: usize,
    identity: Option<PresenceState>,
) -> Result<(), String> {
    use crate::asr::volcengine_speaker_ledger::{EvidenceKind, SpeakerSpan};
    if new_chunks == 0 || new_chunks > scores.len() || scores.len() > PresenceAudioBuffer::WINDOW_CHUNKS
        || scores.iter().any(|score| !score.is_finite() || !(0.0..=1.0).contains(score))
    { return Err("invalid personal activity source window".into()); }
    let chunk = PresenceAudioBuffer::CHUNK_SAMPLES as u64;
    let context_samples = (scores.len()-new_chunks) as u64 * chunk;
    let first_end = first_new_end_samples.checked_sub(context_samples)
        .ok_or("personal activity context precedes its source")?;
    let start = first_end.checked_sub(chunk).ok_or("personal activity starts before its source")?;
    let end = first_new_end_samples.checked_add((new_chunks-1) as u64*chunk)
        .ok_or("personal activity source window overflowed")?;
    let kind = |state| match state {
        PresenceState::Owner => EvidenceKind::Owner,
        PresenceState::ConfirmedOther => EvidenceKind::Other,
        PresenceState::Unknown => EvidenceKind::Unknown,
    };
    timeline.note_identity(SpeakerSpan {start_ms:start/16,end_ms:end/16,
        kind:kind(identity.unwrap_or(PresenceState::Unknown))});
    for (index, probability) in scores.iter().enumerate() {
        let end = first_end+index as u64*chunk;
        let state = fuse_presence(*probability,identity);
        if *probability >= OWNER_ACTIVITY_MIN_PROBABILITY {
            timeline.note_presence(SpeakerSpan {start_ms:(end-chunk)/16,end_ms:end/16,kind:EvidenceKind::Owner});
        }
        if index >= scores.len()-new_chunks || state == PresenceState::ConfirmedOther {
            timeline.note_presence(SpeakerSpan {start_ms:(end-chunk)/16,end_ms:end/16,kind:kind(state)});
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct PresenceObservation {
    pub analyzed_end_ms: u64,
    pub last_positive_end_ms: Option<u64>,
    /// Preserve unresolved coverage even when a consumer misses a snapshot.
    pub last_unresolved_end_ms: Option<u64>,
    /// True only when the producer reconciled its complete source history.
    /// Partial snapshots cannot erase previously observed unresolved speech.
    pub source_history_complete: bool,
    pub state: PresenceState,
    pub observed_at: Instant,
}

/// Independently completed silence. This carries no speaker identity and is
/// never constructed from a missing personal-model result or provider text.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ConfirmedSilenceObservation {
    pub analyzed_end_ms: u64,
    pub silence_since_ms: u64,
    pub observed_at: Instant,
}

/// One endpoint allowance per genuine owner advance. Repeated background text
/// receipts cannot buy new continuation windows. Unknown/stalled inference is
/// deliberately separate from completed negative observations.
#[derive(Debug, Default)]
pub(crate) struct PresenceEndpoint {
    latest: Option<PresenceObservation>,
    last_unresolved_end_ms: Option<u64>,
    delivery_at: Option<Instant>,
    delivered_continuation_end_ms: Option<u64>,
    pending_delivery: Option<PendingPresenceDelivery>,
    invalidated: bool,
}

#[derive(Debug)]
struct PendingPresenceDelivery {
    activity_end_ms: u64,
    submitted_at: Instant,
}

impl PresenceEndpoint {
    pub(crate) fn observe(&mut self, observation: PresenceObservation) -> bool {
        if self
            .latest
            .is_some_and(|old| observation.analyzed_end_ms <= old.analyzed_end_ms)
        {
            return false;
        }
        let owner_advanced =
            observation.last_positive_end_ms > self.latest.and_then(|old| old.last_positive_end_ms);
        let unresolved = observation.last_unresolved_end_ms
            .max((observation.state == PresenceState::Unknown).then_some(observation.analyzed_end_ms));
        self.last_unresolved_end_ms = if observation.source_history_complete {
            unresolved
        } else {
            self.last_unresolved_end_ms.max(unresolved)
        };
        self.latest = Some(observation);
        self.invalidated = false;
        owner_advanced
    }

    pub(crate) fn invalidate(&mut self) {
        self.invalidated = true;
    }

    pub(crate) fn owner_activity_end_ms(&self) -> Option<u64> {
        self.latest.and_then(|frame| frame.last_positive_end_ms)
    }

    /// Endpoint continuation may protect uncertain speech without granting
    /// it speaker identity. Confirmed Other cannot advance this frontier.
    pub(crate) fn continuation_activity_end_ms(&self) -> Option<u64> {
        self.owner_activity_end_ms().max(self.last_unresolved_end_ms)
    }

    pub(crate) fn note_delivery(&mut self, received_at: Instant) {
        self.note_submission(received_at, self.continuation_activity_end_ms(), false);
    }

    /// Bind the allowance to possible owner activity observed at dispatch.
    /// A delayed paste cannot consume audio captured after its dispatch.
    pub(crate) fn note_submission(
        &mut self,
        submitted_at: Instant,
        activity_end: Option<u64>,
        receipt_pending: bool,
    ) {
        // The coordinator supersedes older handles when a newer dispatch is
        // accepted. Replace its reservation even when source reconciliation
        // moved this dispatch's activity range behind an already delivered one.
        self.pending_delivery = if receipt_pending {
            activity_end.filter(|end| Some(*end) > self.delivered_continuation_end_ms)
                .map(|activity_end_ms| PendingPresenceDelivery { activity_end_ms, submitted_at })
        } else { None };
        if !receipt_pending {
            if activity_end.is_some() && activity_end > self.delivered_continuation_end_ms {
                self.delivered_continuation_end_ms = activity_end;
                self.delivery_at = Some(submitted_at);
            }
        }
    }

    /// Only the first confirmed receipt for this continuation range starts
    /// the window. Subsequent confirmed-background bodies cannot renew it.
    pub(crate) fn confirm_delivery(&mut self, received_at: Instant, activity_end: Option<u64>) {
        if self.pending_delivery.as_ref()
            .is_some_and(|pending| Some(pending.activity_end_ms) == activity_end)
        {
            let pending = self.pending_delivery.take().expect("matching pending delivery");
            self.delivered_continuation_end_ms = Some(pending.activity_end_ms);
            self.delivery_at = Some(pending.submitted_at.max(received_at));
        }
    }

    /// An unobserved/failed editor transaction is terminal, not silence or a
    /// receipt. It releases only its matching reservation; live owner/unknown
    /// activity and the last confirmed delivery keep their original authority.
    pub(crate) fn settle_unconfirmed_delivery(&mut self, activity_end: Option<u64>) {
        if self.pending_delivery.as_ref()
            .is_some_and(|pending| Some(pending.activity_end_ms) == activity_end)
        {
            self.pending_delivery = None;
        }
    }

    pub(crate) fn due(&self, captured_ms: u64, now: Instant, timeout_ms: u64) -> bool {
        let Some(frame) = self.latest else {
            return false;
        };
        let Some(owner_end) = frame.last_positive_end_ms else {
            return false;
        };
        !self.invalidated
            && self.pending_delivery.is_none()
            && frame.state == PresenceState::ConfirmedOther
            && frame.analyzed_end_ms <= captured_ms
            && captured_ms.saturating_sub(frame.analyzed_end_ms) <= 250
            && frame.observed_at <= now
            && now.saturating_duration_since(frame.observed_at) <= Duration::from_millis(500)
            && frame.analyzed_end_ms.saturating_sub(
                owner_end.max(self.last_unresolved_end_ms.unwrap_or_default()),
            ) >= timeout_ms
            && self.delivery_at.is_none_or(|received_at| {
                now.saturating_duration_since(received_at) >= Duration::from_millis(timeout_ms)
            })
    }

    pub(crate) fn due_on_confirmed_silence(
        &self,
        silence: ConfirmedSilenceObservation,
        captured_ms: u64,
        now: Instant,
        timeout_ms: u64,
    ) -> bool {
        let quiet_start = silence.silence_since_ms.max(
            self.latest.and_then(|frame| frame.last_positive_end_ms).unwrap_or_default(),
        );
        silence.analyzed_end_ms <= captured_ms
            && self.pending_delivery.is_none()
            && captured_ms.saturating_sub(silence.analyzed_end_ms) <= 250
            && silence.observed_at <= now
            && now.saturating_duration_since(silence.observed_at) <= Duration::from_millis(500)
            && silence.analyzed_end_ms.saturating_sub(quiet_start) >= timeout_ms
            && self.delivery_at.is_none_or(|received_at| {
                now.saturating_duration_since(received_at) >= Duration::from_millis(timeout_ms)
            })
    }
}

#[cfg(target_os = "windows")]
#[path = "personal_vad_windows.rs"]
mod windows;
#[cfg(target_os = "windows")]
pub(crate) use windows::{enrollment_reference, PersonalVadReference, PersonalVadStream, ENCODER_SHA};

#[cfg(not(target_os = "windows"))]
pub(crate) struct PersonalVadStream;
#[cfg(not(target_os = "windows"))]
impl PersonalVadStream {
    pub(crate) fn start(_: Vec<u8>) -> Self {
        Self
    }
    pub(crate) fn submit(&self, _: &[u8], _: u64) {}
    pub(crate) fn snapshot(&self) -> Option<PresenceObservation> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_editor_reservation_keeps_prior_confirmed_clock_and_newer_range_ownership() {
        let now = Instant::now();
        let mut endpoint = PresenceEndpoint::default();
        endpoint.note_submission(now, Some(4_000), false);
        let later = now + Duration::from_secs(2);
        endpoint.note_submission(later, Some(6_000), true);
        assert_eq!(endpoint.delivery_at, Some(now));
        assert_eq!(endpoint.delivered_continuation_end_ms, Some(4_000));
        endpoint.settle_unconfirmed_delivery(Some(4_000));
        assert!(endpoint.pending_delivery.is_some(), "an older result cannot retire a newer range");
        endpoint.settle_unconfirmed_delivery(Some(6_000));
        assert!(endpoint.pending_delivery.is_none());
        assert_eq!(endpoint.delivery_at, Some(now));
        assert_eq!(endpoint.delivered_continuation_end_ms, Some(4_000));
        endpoint.confirm_delivery(later, Some(6_000));
        assert_eq!(endpoint.delivery_at, Some(now), "a retired transaction cannot manufacture confirmation");
        endpoint.note_submission(later, Some(6_000), true);
        endpoint.confirm_delivery(later + Duration::from_millis(50), Some(6_000));
        assert_eq!(endpoint.delivery_at, Some(later + Duration::from_millis(50)));
        assert_eq!(endpoint.delivered_continuation_end_ms, Some(6_000));
        endpoint.note_submission(later, Some(8_000), true);
        endpoint.note_submission(later, Some(6_000), true);
        assert!(endpoint.pending_delivery.is_none(),
            "a superseded higher range cannot retain an orphan wait after source reconciliation");
        assert_eq!(endpoint.delivery_at, Some(later + Duration::from_millis(50)));
    }


    #[test]
    fn pending_word_absence_requires_endpoint_grade_and_retains_positive_context() {
        let mut source = EndpointSourceTimeline::default();
        let mismatch = PresenceIdentityObservation {projection:PresenceState::ConfirmedOther,endpoint:PresenceState::Unknown};
        source.note_scored_window(&[0.1;15],160*16,15,Some(mismatch)).unwrap();
        assert!(!source.confirms_other(100,2200), "coarse word Other cannot clear pending own speech");
        let strong = PresenceIdentityObservation {projection:PresenceState::ConfirmedOther,endpoint:PresenceState::ConfirmedOther};
        source.note_scored_window(&[0.1;15],160*16,15,Some(strong)).unwrap();
        assert!(source.confirms_other(100,2200));
        assert!(!source.confirms_other(100,2500), "unobserved tail remains protected");
        let mut positive = [0.1;15]; positive[8] = 0.8;
        source.note_scored_window(&positive,160*16,15,Some(strong)).unwrap();
        assert!(!source.confirms_other(100,2200), "a detector positive revokes exact source absence");
    }

    #[test]
    fn qualified_context_updates_its_original_pending_chunks_and_preserves_owner_vetoes() {
        use crate::asr::volcengine_speaker_ledger::{EvidenceKind,LocalSpeakerTimeline,SpeakerSpan};
        let mut source=LocalSpeakerTimeline::default();
        source.note_presence(SpeakerSpan{start_ms:0,end_ms:320,kind:EvidenceKind::Unknown});
        publish_scored_window(&mut source,&[0.1,0.1,0.1],480*16,1,None).unwrap();
        assert_eq!(source.unresolved_activity_end_ms_after(0),Some(480),"rejection without identity remains pending");
        publish_scored_window(&mut source,&[0.1,0.1,0.1],480*16,1,Some(PresenceState::ConfirmedOther)).unwrap();
        assert_eq!(source.unresolved_activity_end_ms_after(0),None,"later qualified context resolves those exact chunks");
        publish_scored_window(&mut source,&[0.8,0.1,0.1],480*16,1,Some(PresenceState::ConfirmedOther)).unwrap();
        assert_eq!(source.unresolved_activity_end_ms_after(0),Some(160),"old target evidence keeps its original time and wins");
        assert!(publish_scored_window(&mut source,&[0.1;3],160*16,1,Some(PresenceState::ConfirmedOther)).is_err());
    }

    #[test]
    fn only_complete_history_revisions_can_retire_unknown_and_actual_receipts_still_get_three_seconds() {
        let now=Instant::now();
        let mut endpoint=PresenceEndpoint::default();
        endpoint.observe(frame(1_000,1_000,true,now));
        let mut pending=frame(4_000,1_000,false,now+Duration::from_millis(3_000));
        pending.state=PresenceState::Unknown;
        pending.last_unresolved_end_ms=Some(4_000);
        endpoint.observe(pending);
        let partial=frame(5_000,1_000,false,now+Duration::from_millis(4_000));
        endpoint.observe(partial);
        assert!(!endpoint.due(5_000,partial.observed_at,3_000),"a partial snapshot cannot erase the missed owner possibility");
        let mut reconciled=frame(5_160,1_000,false,now+Duration::from_millis(4_160));
        reconciled.source_history_complete=true;
        endpoint.observe(reconciled);
        assert!(endpoint.due(5_160,reconciled.observed_at,3_000));
        let received=reconciled.observed_at;
        // A receipt that belonged to the pending owner possibility must keep
        // its real wall-clock continuation even after identity is reconciled.
        endpoint.note_submission(received,Some(4_000),false);
        let mut next=frame(8_000,1_000,false,received+Duration::from_millis(2_840));
        next.source_history_complete=true;
        endpoint.observe(next);
        assert!(!endpoint.due(8_000,next.observed_at,3_000));
        next.analyzed_end_ms=8_160; next.observed_at=received+Duration::from_millis(3_000);
        endpoint.observe(next);
        assert!(endpoint.due(8_160,next.observed_at,3_000));
    }

    #[test]
    fn fractional_millisecond_packets_preserve_the_exact_sample_frontier() {
        let mut audio = PresenceAudioBuffer::default();
        let mut end = 37u64;
        let mut expected = Vec::new();
        let mut processed = Vec::new();
        for (index, samples) in [1usize, 17, 63, 288, 31, 1599, 2560, 13, 3071].into_iter().enumerate() {
            let pcm = vec![index as u8 + 1; samples*2];
            expected.extend_from_slice(&pcm);
            end += samples as u64;
            audio.push(&pcm, end).unwrap();
            if index % 3 == 2 {
                while let Some(window) = audio.next_window() {
                    let bytes = window.new_chunks * PresenceAudioBuffer::CHUNK_SAMPLES * 2;
                    assert_eq!(window.first_new_end_samples, 37 + processed.len() as u64 / 2 + 2560);
                    processed.extend_from_slice(&window.pcm[window.pcm.len()-bytes..]);
                }
            }
        }
        while let Some(window) = audio.next_window() {
            let bytes = window.new_chunks * PresenceAudioBuffer::CHUNK_SAMPLES * 2;
            processed.extend_from_slice(&window.pcm[window.pcm.len()-bytes..]);
        }
        assert_eq!(processed, expected[..expected.len()/5120*5120]);
        assert_eq!(audio.pending, expected[processed.len()..]);
    }

    #[test]
    fn accumulated_audio_is_scored_in_order_without_repeating_the_old_work() {
        let mut audio = PresenceAudioBuffer::default();
        let pcm = (0..22usize).flat_map(|index| vec![index as u8; 5120]).collect::<Vec<_>>();
        audio.push(&pcm, (pcm.len()/2) as u64).unwrap();
        let first = audio.next_window().unwrap();
        assert_eq!(first.new_chunks, 15);
        assert_eq!(first.pcm, pcm[..15*5120]);
        let second = audio.next_window().unwrap();
        assert_eq!(second.new_chunks, 7);
        assert_eq!(second.first_new_end_samples, 16*2560);
        assert_eq!(second.pcm, pcm[7*5120..]);
        assert!(audio.next_window().is_none());
    }

    #[test]
    fn source_gaps_and_reordered_samples_never_become_negative_coverage() {
        let mut audio = PresenceAudioBuffer::default();
        audio.push(&vec![1; 5120], 3000).unwrap();
        assert!(audio.push(&vec![2; 5120], 5561).is_err());
        assert!(audio.push(&vec![2; 5120], 3000).is_err());
        assert!(audio.push(&[1,2,3], 3001).is_err());
        let first = audio.next_window().unwrap();
        assert_eq!(first.pcm, vec![1; 5120]);
        assert!(audio.next_window().is_none());
        audio.push(&vec![2; 5120], 5560).unwrap();
        assert_eq!(audio.next_window().unwrap().first_new_end_samples, 5560);
    }

    fn frame(audio: u64, owner: u64, positive: bool, now: Instant) -> PresenceObservation {
        PresenceObservation {
            analyzed_end_ms: audio,
            last_positive_end_ms: Some(owner),
            last_unresolved_end_ms: None,
            source_history_complete: false,
            state: if positive { PresenceState::Owner } else { PresenceState::ConfirmedOther },
            observed_at: now,
        }
    }

    #[test]
    fn continuous_background_and_repeated_receipts_cannot_extend_owner_clock() {
        let now = Instant::now();
        let mut endpoint = PresenceEndpoint::default();
        endpoint.observe(frame(1_120, 1_120, true, now));
        endpoint.note_delivery(now);
        for offset in [1_000, 2_000, 3_000, 4_000] {
            let observed = now + Duration::from_millis(offset);
            endpoint.observe(frame(1_120 + offset, 1_120, false, observed));
            endpoint.note_delivery(observed);
            assert_eq!(
                endpoint.due(1_120 + offset, observed, 3_000),
                offset >= 3_000
            );
        }
    }

    #[test]
    fn owner_return_and_actual_editor_receipt_earn_a_fresh_three_seconds() {
        let now = Instant::now();
        let mut endpoint = PresenceEndpoint::default();
        endpoint.observe(frame(1_000, 1_000, true, now));
        endpoint.note_delivery(now);
        let resumed = now + Duration::from_millis(2_800);
        assert!(endpoint.observe(frame(3_800, 3_800, true, resumed)));
        assert!(!endpoint.due(3_800, resumed, 3_000));
        let received = resumed + Duration::from_millis(500);
        endpoint.note_delivery(received);
        let quiet = received + Duration::from_millis(3_000);
        endpoint.observe(frame(7_300, 3_800, false, quiet));
        assert!(!endpoint.due(7_300, quiet - Duration::from_millis(1), 3_000));
        assert!(endpoint.due(7_300, quiet, 3_000));
    }

    #[test]
    fn unknown_lag_and_replayed_owner_frames_are_not_absence() {
        let now = Instant::now();
        let mut endpoint = PresenceEndpoint::default();
        assert!(!endpoint.due(10_000, now, 3_000));
        endpoint.observe(frame(4_000, 1_000, false, now));
        assert!(!endpoint.due(4_251, now, 3_000));
        assert!(!endpoint.due(4_000, now + Duration::from_millis(501), 3_000));
        assert!(!endpoint.observe(frame(1_000, 1_000, true, now + Duration::from_millis(10))));
        assert!(endpoint.due(4_000, now, 3_000));
    }

    #[test]
    fn future_analysis_and_observation_cannot_propose_stop() {
        let now = Instant::now();
        let mut endpoint = PresenceEndpoint::default();
        endpoint.observe(frame(4_160, 1_000, false, now));
        assert!(!endpoint.due(4_000, now, 3_000));
        assert!(endpoint.due(4_160, now, 3_000));
        let next = now + Duration::from_millis(1);
        endpoint.observe(frame(4_320, 1_000, false, next));
        assert!(!endpoint.due(4_320, now, 3_000));
        assert!(endpoint.due(4_320, next, 3_000));
        let silence = ConfirmedSilenceObservation {
            analyzed_end_ms: 4_320, silence_since_ms: 1_000, observed_at: next,
        };
        assert!(!endpoint.due_on_confirmed_silence(silence, 4_320, now, 3_000));
        assert!(endpoint.due_on_confirmed_silence(silence, 4_320, next, 3_000));
    }

    #[test]
    fn target_rejection_requires_independent_other_evidence_and_owner_wins_conflict() {
        for probability in [0.0, 0.1, 0.426270, 0.699] {
            assert_eq!(fuse_presence(probability, None), PresenceState::Unknown);
            assert_eq!(fuse_presence(probability, Some(PresenceState::Unknown)), PresenceState::Unknown);
            assert_eq!(fuse_presence(probability, Some(PresenceState::Owner)), PresenceState::Owner);
            assert_eq!(fuse_presence(probability, Some(PresenceState::ConfirmedOther)), PresenceState::ConfirmedOther);
        }
        assert_eq!(fuse_presence(0.8, Some(PresenceState::ConfirmedOther)), PresenceState::Owner);
    }

    #[test]
    fn unresolved_owner_speech_cannot_be_counted_as_an_other_speaker_quiet_span() {
        let start = Instant::now();
        let mut endpoint = PresenceEndpoint::default();
        endpoint.observe(frame(12_960, 12_960, true, start));
        endpoint.note_delivery(start);
        // Real premature-stop shape: the target head stays low while the owner
        // keeps talking. Every completed unresolved window must veto STOP.
        for end in [15_680, 17_760, 20_000, 22_080, 24_160, 26_400] {
            let now = start + Duration::from_millis(end - 12_960);
            let mut observation = frame(end, 12_960, false, now);
            observation.state = PresenceState::Unknown;
            endpoint.observe(observation);
            assert!(!endpoint.due(end, now, 3_000));
        }
        for end in [27_400, 29_399, 29_400] {
            let now = start + Duration::from_millis(end - 12_960);
            endpoint.observe(frame(end, 12_960, false, now));
            assert_eq!(endpoint.due(end, now, 3_000), end == 29_400);
        }
        // Independently completed actual silence still ends normally without
        // requiring an identity score on silence.
        let quiet_at = start + Duration::from_millis(16_440);
        assert!(endpoint.due_on_confirmed_silence(ConfirmedSilenceObservation {
            analyzed_end_ms: 29_400, silence_since_ms: 26_400, observed_at: quiet_at,
        }, 29_400, quiet_at, 3_000));
    }

    #[test]
    fn missed_unresolved_snapshot_still_breaks_proven_other_coverage() {
        let start = Instant::now();
        let mut endpoint = PresenceEndpoint::default();
        endpoint.observe(frame(1_000, 1_000, true, start));
        let observed = start + Duration::from_millis(3_500);
        let mut latest = frame(4_500, 1_000, false, observed);
        latest.last_unresolved_end_ms = Some(4_000);
        endpoint.observe(latest);
        assert!(!endpoint.due(4_500, observed, 3_000));
        let now = start + Duration::from_millis(6_000);
        let mut latest = frame(7_000, 1_000, false, now);
        latest.last_unresolved_end_ms = Some(4_000);
        endpoint.observe(latest);
        assert!(endpoint.due(7_000, now, 3_000));
    }

    #[test]
    fn unknown_speech_keeps_the_matching_body_receipt_continuation_window() {
        let start = Instant::now();
        let mut endpoint = PresenceEndpoint::default();
        endpoint.observe(frame(4_000, 4_000, true, start));
        endpoint.note_delivery(start);
        let mut unknown = frame(10_000, 4_000, false, start + Duration::from_millis(6_000));
        unknown.state = PresenceState::Unknown;
        endpoint.observe(unknown);
        // A real body receipt follows unresolved speech. It cannot borrow
        // the old body's already-spent continuation allowance.
        let received = start + Duration::from_millis(6_400);
        endpoint.note_delivery(received);
        let before = received + Duration::from_millis(2_900);
        endpoint.observe(frame(13_200, 4_000, false, before));
        assert!(!endpoint.due(13_300, before, 3_000));
        assert!(endpoint.due(13_400, received + Duration::from_millis(3_000), 3_000));
    }

    #[test]
    fn rolling_context_owner_proof_keeps_its_old_source_time_and_continuation() {
        let mut scores = [0.1; 15];
        scores[0] = 0.8;
        let owner_end = latest_target_source_end(&scores, 4_000*16, 1).unwrap()/16;
        assert_eq!(owner_end, 1_760, "old context cannot masquerade as current owner activity");
        let now = Instant::now();
        let mut endpoint = PresenceEndpoint::default();
        endpoint.observe(frame(4_000, owner_end, false, now));
        assert!(!endpoint.due(4_000, now, 3_000), "recent context owner still vetoes STOP");
        endpoint.observe(frame(4_800, owner_end, false, now));
        assert!(endpoint.due(4_800, now, 3_000), "confirmed Other retains the original source clock");
        assert_eq!(latest_target_source_end(&scores, 4_000*16, 3), Some(2_080*16));
        assert_eq!(latest_target_source_end(&scores, 0, 1), None);
        assert_eq!(latest_target_source_end(&scores, 4_000*16, 16), None);
    }
}
