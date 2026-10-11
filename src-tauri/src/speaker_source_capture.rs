//! Byte-checked handoff of an enrolled wake into one continuous speaker source.
//!
//! Kept test-only until the joint acoustic/latency gate passes. The coordinator
//! must supply its actual verifier input and capture generation before cropping
//! the cloud body. This adapter never enrolls, adapts, scores or persists audio.

pub(crate) use crate::speaker_channel_binding::SampleRange;
use crate::speaker_channel_binding::{
    MillisecondRange, RetainedVerifiedWakeAudio, RetainedVerifiedWakeSampleAudio, SourceGeneration,
    VerifiedWakeOwnerProof, VerifiedWakeOwnerSampleProof, WakeOwnerPolicy,
};
use sha2::{Digest, Sha256};

const SAMPLES_PER_MS: u64 = 16;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CapturePendingReason {
    SourceMismatch,
    UnqualifiedWake,
    InvalidPcm,
    InvalidRange,
    VerificationBytesMismatch,
    FutureVerification,
    CoordinateOverflow,
    ClockPrecision,
    InitialNotFed,
    InitialAlreadyFed,
    SourceGapOrDuplicate,
    NativeClockMismatch,
    FeedFailed,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum CaptureFeedError<E> {
    Pending(CapturePendingReason),
    Backend(E),
}

pub(crate) struct CapturedPcm<'a> {
    pub(crate) source: SourceGeneration,
    pub(crate) capture_start_sample: u64,
    pub(crate) pcm: &'a [u8],
}

/// Construct this immediately around the exact already-enrolled verifier call.
/// The positive policy comes from that call, not from a phrase hit or ASR ID.
pub(crate) struct VerifiedWakeInput {
    source: SourceGeneration,
    range: SampleRange,
    pcm_sha256: [u8; 32],
    observed_capture_frontier_sample: u64,
}

impl VerifiedWakeInput {
    pub(crate) fn from_verifier_input(
        source: SourceGeneration,
        capture_start_sample: u64,
        pcm: &[u8],
        policy: WakeOwnerPolicy,
        observed_capture_frontier_sample: u64,
    ) -> Result<Self, CapturePendingReason> {
        if policy != WakeOwnerPolicy::EnrolledOwner {
            return Err(CapturePendingReason::UnqualifiedWake);
        }
        let end = capture_start_sample
            .checked_add(pcm_samples(pcm)?)
            .ok_or(CapturePendingReason::CoordinateOverflow)?;
        if observed_capture_frontier_sample < end {
            return Err(CapturePendingReason::FutureVerification);
        }
        Ok(Self {
            source,
            range: SampleRange {
                start: capture_start_sample,
                end,
            },
            pcm_sha256: Sha256::digest(pcm).into(),
            observed_capture_frontier_sample,
        })
    }
}

/// Keeps only the bounded pre-crop candidate until its first feed. Body chunks
/// are validated and fed directly; they are never accumulated as raw storage.
pub(crate) struct RetainedWakeSource {
    source: SourceGeneration,
    capture_origin_sample: u64,
    body_origin_sample: u64,
    next_capture_sample: u64,
    verification: VerifiedWakeInput,
    initial_pcm: Option<Vec<u8>>,
    initial_fed: bool,
    feed_failed: bool,
}

impl RetainedWakeSource {
    pub(crate) fn before_body_crop(
        candidate: CapturedPcm<'_>,
        verification: VerifiedWakeInput,
        body_start_capture_sample: u64,
    ) -> Result<Self, CapturePendingReason> {
        if candidate.source != verification.source {
            return Err(CapturePendingReason::SourceMismatch);
        }
        let end = candidate
            .capture_start_sample
            .checked_add(pcm_samples(candidate.pcm)?)
            .ok_or(CapturePendingReason::CoordinateOverflow)?;
        if verification.range.start < candidate.capture_start_sample
            || verification.range.end > end
            || body_start_capture_sample < candidate.capture_start_sample
            || body_start_capture_sample > end
        {
            return Err(CapturePendingReason::InvalidRange);
        }
        if verification.observed_capture_frontier_sample > end {
            return Err(CapturePendingReason::FutureVerification);
        }
        let start_byte =
            sample_byte_offset(verification.range.start - candidate.capture_start_sample)?;
        let end_byte = sample_byte_offset(verification.range.end - candidate.capture_start_sample)?;
        let actual: [u8; 32] = Sha256::digest(&candidate.pcm[start_byte..end_byte]).into();
        if actual != verification.pcm_sha256 {
            return Err(CapturePendingReason::VerificationBytesMismatch);
        }
        Ok(Self {
            source: candidate.source,
            capture_origin_sample: candidate.capture_start_sample,
            body_origin_sample: body_start_capture_sample - candidate.capture_start_sample,
            next_capture_sample: end,
            verification,
            initial_pcm: Some(candidate.pcm.to_vec()),
            initial_fed: false,
            feed_failed: false,
        })
    }

    pub(crate) fn feed_initial<E>(
        &mut self,
        push: impl FnOnce(&[u8]) -> Result<(), E>,
    ) -> Result<SampleRange, CaptureFeedError<E>> {
        self.check_live().map_err(CaptureFeedError::Pending)?;
        let pcm = self.initial_pcm.take().ok_or(CaptureFeedError::Pending(
            CapturePendingReason::InitialAlreadyFed,
        ))?;
        // A backend error may have consumed some input. Retire this source;
        // never retry the same bytes and accidentally duplicate the wake.
        if let Err(error) = push(&pcm) {
            self.feed_failed = true;
            return Err(CaptureFeedError::Backend(error));
        }
        self.initial_fed = true;
        Ok(SampleRange {
            start: 0,
            end: self.received_samples(),
        })
    }

    pub(crate) fn feed_body<E>(
        &mut self,
        chunk: CapturedPcm<'_>,
        push: impl FnOnce(&[u8]) -> Result<(), E>,
    ) -> Result<SampleRange, CaptureFeedError<E>> {
        self.check_live().map_err(CaptureFeedError::Pending)?;
        if !self.initial_fed {
            return Err(CaptureFeedError::Pending(
                CapturePendingReason::InitialNotFed,
            ));
        }
        if chunk.source != self.source {
            return Err(CaptureFeedError::Pending(
                CapturePendingReason::SourceMismatch,
            ));
        }
        if chunk.capture_start_sample != self.next_capture_sample {
            return Err(CaptureFeedError::Pending(
                CapturePendingReason::SourceGapOrDuplicate,
            ));
        }
        let samples = pcm_samples(chunk.pcm).map_err(CaptureFeedError::Pending)?;
        let end =
            self.next_capture_sample
                .checked_add(samples)
                .ok_or(CaptureFeedError::Pending(
                    CapturePendingReason::CoordinateOverflow,
                ))?;
        let start = self.received_samples();
        if let Err(error) = push(chunk.pcm) {
            self.feed_failed = true;
            return Err(CaptureFeedError::Backend(error));
        }
        self.next_capture_sample = end;
        Ok(SampleRange {
            start,
            end: self.received_samples(),
        })
    }

    pub(crate) fn received_samples(&self) -> u64 {
        self.next_capture_sample - self.capture_origin_sample
    }

    pub(crate) fn binding_inputs(
        &self,
        native_received_samples: u64,
    ) -> Result<(VerifiedWakeOwnerProof, RetainedVerifiedWakeAudio), CapturePendingReason> {
        let (sample_proof, sample_retained) =
            self.binding_sample_inputs(native_received_samples)?;
        // Compatibility with whole-ms callers only. The sample binder below
        // is the authoritative path for fractional capture and crop origins.
        let capture_range = MillisecondRange {
            start_ms: exact_ms(sample_proof.capture_range.start)?,
            end_ms: exact_ms(sample_proof.capture_range.end)?,
        };
        Ok((
            VerifiedWakeOwnerProof {
                source: sample_proof.source,
                policy: sample_proof.policy,
                capture_range,
                pcm_sha256: sample_proof.pcm_sha256,
                observed_at_input_ms: exact_ms(sample_proof.observed_at_input_sample)?,
            },
            RetainedVerifiedWakeAudio {
                source: sample_retained.source,
                capture_range,
                continuous_range: MillisecondRange {
                    start_ms: exact_ms(sample_retained.continuous_range.start)?,
                    end_ms: exact_ms(sample_retained.continuous_range.end)?,
                },
                pcm_sha256: sample_retained.pcm_sha256,
                occurrence_count: sample_retained.occurrence_count,
                source_gap_free: sample_retained.source_gap_free,
                body_origin_ms: exact_ms(sample_retained.body_origin_sample)?,
            },
        ))
    }

    pub(crate) fn binding_sample_inputs(
        &self,
        native_received_samples: u64,
    ) -> Result<
        (
            VerifiedWakeOwnerSampleProof,
            RetainedVerifiedWakeSampleAudio,
        ),
        CapturePendingReason,
    > {
        self.check_live()?;
        if !self.initial_fed {
            return Err(CapturePendingReason::InitialNotFed);
        }
        if native_received_samples != self.received_samples() {
            return Err(CapturePendingReason::NativeClockMismatch);
        }
        let continuous_range = SampleRange {
            start: self.verification.range.start - self.capture_origin_sample,
            end: self.verification.range.end - self.capture_origin_sample,
        };
        let proof = VerifiedWakeOwnerSampleProof {
            source: self.source,
            policy: WakeOwnerPolicy::EnrolledOwner,
            capture_range: self.verification.range,
            pcm_sha256: self.verification.pcm_sha256,
            observed_at_input_sample: self.verification.observed_capture_frontier_sample
                - self.capture_origin_sample,
        };
        let retained = RetainedVerifiedWakeSampleAudio {
            source: self.source,
            capture_range: self.verification.range,
            continuous_range,
            pcm_sha256: self.verification.pcm_sha256,
            occurrence_count: 1,
            source_gap_free: true,
            body_origin_sample: self.body_origin_sample,
        };
        Ok((proof, retained))
    }

    pub(crate) fn map_body_word_samples(
        &self,
        source: SourceGeneration,
        range_ms: MillisecondRange,
    ) -> Result<SampleRange, CapturePendingReason> {
        self.check_live()?;
        if !self.initial_fed {
            return Err(CapturePendingReason::InitialNotFed);
        }
        if source != self.source {
            return Err(CapturePendingReason::SourceMismatch);
        }
        if range_ms.start_ms >= range_ms.end_ms {
            return Err(CapturePendingReason::InvalidRange);
        }
        let map = |ms: u64| {
            ms.checked_mul(SAMPLES_PER_MS)
                .and_then(|samples| self.body_origin_sample.checked_add(samples))
                .ok_or(CapturePendingReason::CoordinateOverflow)
        };
        let range = SampleRange {
            start: map(range_ms.start_ms)?,
            end: map(range_ms.end_ms)?,
        };
        if range.end > self.received_samples() {
            return Err(CapturePendingReason::FutureVerification);
        }
        Ok(range)
    }

    fn check_live(&self) -> Result<(), CapturePendingReason> {
        if self.feed_failed {
            Err(CapturePendingReason::FeedFailed)
        } else {
            Ok(())
        }
    }
}

fn pcm_samples(pcm: &[u8]) -> Result<u64, CapturePendingReason> {
    if pcm.is_empty() || pcm.len() % 2 != 0 {
        return Err(CapturePendingReason::InvalidPcm);
    }
    u64::try_from(pcm.len() / 2).map_err(|_| CapturePendingReason::CoordinateOverflow)
}

fn sample_byte_offset(samples: u64) -> Result<usize, CapturePendingReason> {
    samples
        .checked_mul(2)
        .and_then(|bytes| usize::try_from(bytes).ok())
        .ok_or(CapturePendingReason::CoordinateOverflow)
}

fn exact_ms(samples: u64) -> Result<u64, CapturePendingReason> {
    if samples % SAMPLES_PER_MS != 0 {
        return Err(CapturePendingReason::ClockPrecision);
    }
    Ok(samples / SAMPLES_PER_MS)
}

#[cfg(test)]
mod tests {
    use super::*;
    const SOURCE: SourceGeneration = SourceGeneration {
        source_id: 1,
        capture_id: 2,
        generation: 3,
    };

    fn source(pcm: &[u8], body: u64) -> RetainedWakeSource {
        let proof = VerifiedWakeInput::from_verifier_input(
            SOURCE,
            0,
            &pcm[..64],
            WakeOwnerPolicy::EnrolledOwner,
            32,
        )
        .unwrap();
        RetainedWakeSource::before_body_crop(
            CapturedPcm {
                source: SOURCE,
                capture_start_sample: 0,
                pcm,
            },
            proof,
            body,
        )
        .unwrap()
    }

    #[test]
    fn initial_wake_and_overlapping_body_are_fed_once_with_exact_word_mapping() {
        let pcm: Vec<_> = (0..128).collect();
        let mut source = source(&pcm, 16);
        assert_eq!(
            source.binding_inputs(0).unwrap_err(),
            CapturePendingReason::InitialNotFed
        );
        let mut delivered = Vec::new();
        source
            .feed_initial(|p| {
                delivered.extend_from_slice(p);
                Ok::<_, ()>(())
            })
            .unwrap();
        let tail = [123; 32];
        source
            .feed_body(
                CapturedPcm {
                    source: SOURCE,
                    capture_start_sample: 64,
                    pcm: &tail,
                },
                |p| {
                    delivered.extend_from_slice(p);
                    Ok::<_, ()>(())
                },
            )
            .unwrap();
        assert_eq!(delivered, [pcm, tail.to_vec()].concat());
        let (proof, retained) = source.binding_inputs(80).unwrap();
        assert_eq!(
            proof.capture_range,
            MillisecondRange {
                start_ms: 0,
                end_ms: 2
            }
        );
        assert_eq!(retained.body_origin_ms, 1);
        assert_eq!(retained.occurrence_count, 1);
        assert_eq!(
            source
                .map_body_word_samples(
                    SOURCE,
                    MillisecondRange {
                        start_ms: 1,
                        end_ms: 3
                    }
                )
                .unwrap(),
            SampleRange { start: 32, end: 64 }
        );
        assert_eq!(
            source.feed_initial(|_| Ok::<_, ()>(())).unwrap_err(),
            CaptureFeedError::Pending(CapturePendingReason::InitialAlreadyFed)
        );
        assert_eq!(
            source.binding_inputs(81).unwrap_err(),
            CapturePendingReason::NativeClockMismatch
        );
    }

    #[test]
    fn source_gap_duplicate_or_generation_change_never_reaches_the_backend() {
        let mut source = source(&[0; 128], 16);
        source.feed_initial(|_| Ok::<_, ()>(())).unwrap();
        for start in [63, 65] {
            assert_eq!(
                source
                    .feed_body(
                        CapturedPcm {
                            source: SOURCE,
                            capture_start_sample: start,
                            pcm: &[0; 32]
                        },
                        |_| panic!("invalid bytes must not be fed")
                    )
                    .unwrap_err(),
                CaptureFeedError::<()>::Pending(CapturePendingReason::SourceGapOrDuplicate)
            );
        }
        assert_eq!(
            source
                .feed_body(
                    CapturedPcm {
                        source: SourceGeneration {
                            generation: 4,
                            ..SOURCE
                        },
                        capture_start_sample: 64,
                        pcm: &[0; 32]
                    },
                    |_| panic!("stale source must not be fed")
                )
                .unwrap_err(),
            CaptureFeedError::<()>::Pending(CapturePendingReason::SourceMismatch)
        );
        assert_eq!(source.received_samples(), 64);
    }

    #[test]
    fn backend_failure_retires_the_source_instead_of_retrying_partially_consumed_audio() {
        let mut first = source(&[0; 128], 16);
        assert_eq!(
            first.feed_initial(|_| Err("partial feed")).unwrap_err(),
            CaptureFeedError::Backend("partial feed")
        );
        assert_eq!(
            first.binding_inputs(64).unwrap_err(),
            CapturePendingReason::FeedFailed
        );
        let mut body = source(&[0; 128], 16);
        body.feed_initial(|_| Ok::<_, &str>(())).unwrap();
        assert_eq!(
            body.feed_body(
                CapturedPcm {
                    source: SOURCE,
                    capture_start_sample: 64,
                    pcm: &[0; 32]
                },
                |_| Err("partial body")
            )
            .unwrap_err(),
            CaptureFeedError::Backend("partial body")
        );
        assert_eq!(
            body.feed_body(
                CapturedPcm {
                    source: SOURCE,
                    capture_start_sample: 64,
                    pcm: &[0; 32]
                },
                |_| panic!("failed source is retired")
            )
            .unwrap_err(),
            CaptureFeedError::<()>::Pending(CapturePendingReason::FeedFailed)
        );
    }

    #[test]
    fn cropped_or_changed_verifier_bytes_cannot_construct_retained_proof() {
        let pcm = [0; 128];
        for (start, candidate) in [(16, &pcm[32..]), (0, &[1; 128][..])] {
            let proof = VerifiedWakeInput::from_verifier_input(
                SOURCE,
                0,
                &pcm[..64],
                WakeOwnerPolicy::EnrolledOwner,
                32,
            )
            .unwrap();
            assert!(RetainedWakeSource::before_body_crop(
                CapturedPcm {
                    source: SOURCE,
                    capture_start_sample: start,
                    pcm: candidate
                },
                proof,
                32
            )
            .is_err());
        }
        for policy in [
            WakeOwnerPolicy::OpenUnenrolled,
            WakeOwnerPolicy::OpenInactiveProfile,
            WakeOwnerPolicy::EnrolledNonMatch,
        ] {
            assert!(matches!(
                VerifiedWakeInput::from_verifier_input(SOURCE, 0, &pcm[..64], policy, 32),
                Err(CapturePendingReason::UnqualifiedWake)
            ));
        }
    }

    #[test]
    fn fractional_clock_keeps_exact_samples_and_never_rounds_into_identity_authority() {
        let mut source = source(&[0; 128], 17);
        source.feed_initial(|_| Ok::<_, ()>(())).unwrap();
        assert_eq!(
            source.binding_inputs(64).unwrap_err(),
            CapturePendingReason::ClockPrecision
        );
        let (_, retained) = source.binding_sample_inputs(64).unwrap();
        assert_eq!(retained.body_origin_sample, 17);
        assert_eq!(
            source
                .map_body_word_samples(
                    SOURCE,
                    MillisecondRange {
                        start_ms: 0,
                        end_ms: 1
                    }
                )
                .unwrap(),
            SampleRange { start: 17, end: 33 }
        );
    }

    #[test]
    fn shifted_capture_origin_and_partial_ms_verifier_bind_in_exact_samples() {
        use crate::speaker_channel_binding::{
            bind_verified_wake_owner_channel_samples, AnonymousSampleSpan,
            NativeWakeCoreSampleEvidence, VerifiedWakePendingReason,
        };
        let pcm: Vec<u8> = (0..128).collect();
        let proof = VerifiedWakeInput::from_verifier_input(
            SOURCE,
            4,
            &pcm[2..66],
            WakeOwnerPolicy::EnrolledOwner,
            67,
        )
        .unwrap();
        let mut source = RetainedWakeSource::before_body_crop(
            CapturedPcm {
                source: SOURCE,
                capture_start_sample: 3,
                pcm: &pcm,
            },
            proof,
            22,
        )
        .unwrap();
        source.feed_initial(|_| Ok::<_, ()>(())).unwrap();
        let (proof, retained) = source.binding_sample_inputs(64).unwrap();
        assert_eq!(proof.capture_range, SampleRange { start: 4, end: 36 });
        assert_eq!(retained.continuous_range, SampleRange { start: 1, end: 33 });
        assert_eq!(retained.body_origin_sample, 19);
        let spans = [AnonymousSampleSpan {
            channel: 1,
            range: SampleRange { start: 16, end: 32 },
        }];
        let native = |settled| NativeWakeCoreSampleEvidence {
            source: SOURCE,
            spans: &spans,
            settled_through_sample: settled,
            observed_at_input_sample: 64,
        };
        assert_eq!(
            bind_verified_wake_owner_channel_samples(proof, retained, native(32), 64),
            Err(VerifiedWakePendingReason::UnsettledNativeEvidence)
        );
        let bound =
            bind_verified_wake_owner_channel_samples(proof, retained, native(33), 64).unwrap();
        assert_eq!(
            bound
                .map_body_range(
                    SOURCE,
                    MillisecondRange {
                        start_ms: 0,
                        end_ms: 1
                    },
                    64
                )
                .unwrap(),
            SampleRange { start: 19, end: 35 }
        );
        assert_eq!(
            source.binding_inputs(64).unwrap_err(),
            CapturePendingReason::ClockPrecision
        );
    }

    #[test]
    fn invalid_future_or_overflowed_coordinates_remain_pending() {
        assert!(matches!(
            VerifiedWakeInput::from_verifier_input(
                SOURCE,
                u64::MAX,
                &[0; 32],
                WakeOwnerPolicy::EnrolledOwner,
                u64::MAX
            ),
            Err(CapturePendingReason::CoordinateOverflow)
        ));
        assert!(matches!(
            VerifiedWakeInput::from_verifier_input(
                SOURCE,
                0,
                &[0; 64],
                WakeOwnerPolicy::EnrolledOwner,
                31
            ),
            Err(CapturePendingReason::FutureVerification)
        ));
        let mut source = source(&[0; 128], 16);
        source.feed_initial(|_| Ok::<_, ()>(())).unwrap();
        assert_eq!(
            source
                .map_body_word_samples(
                    SOURCE,
                    MillisecondRange {
                        start_ms: 3,
                        end_ms: 4
                    }
                )
                .unwrap_err(),
            CapturePendingReason::FutureVerification
        );
        assert_eq!(
            source
                .map_body_word_samples(
                    SOURCE,
                    MillisecondRange {
                        start_ms: 0,
                        end_ms: u64::MAX
                    }
                )
                .unwrap_err(),
            CapturePendingReason::CoordinateOverflow
        );
    }
}
