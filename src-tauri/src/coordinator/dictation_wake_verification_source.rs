// Exact, session-local input of a direct protected wake verification. This
// receipt supplies provenance to a future diarization source; it grants no
// transcript permission and never changes the wake admission policy.
#[derive(Clone)]
struct WakeVerifierSnapshot {
    candidate_id: u64,
    range: Option<crate::observability::CandidateRange>,
    pcm: Arc<[u8]>,
    pcm_sha256: [u8; 32],
    source_facts: Vec<crate::observability::CandidateSourceRunFact>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct WakeCaptureKey {
    capture_generation: u64,
    segment_id: u32,
    collector_instance_id: u64,
    segment_ordinal: u64,
}

struct RetainedWakeVerificationSource {
    candidate_id: u64,
    capture: WakeCaptureKey,
    verifier_range: crate::observability::CandidateRange,
    verifier_pcm_sha256: [u8; 32],
    original_range: crate::observability::CandidateRange,
    original_pcm: Arc<[u8]>,
    original_source_facts: Vec<crate::observability::CandidateSourceRunFact>,
    // Actual actor input frontier when the verifier result was read. This is
    // neither an inferred wall-clock audio position nor a native model clock.
    observed_at: Instant,
}

struct SessionWakeVerificationSource {
    source_stream_id: u64,
    provider_origin_candidate_byte: u64,
    retained: RetainedWakeVerificationSource,
}

impl RetainedWakeVerificationSource {
    fn for_terminal_body(
        self,
        candidate_id: u64,
        range: Option<crate::observability::CandidateRange>,
        pcm: &[u8],
    ) -> Option<Self> {
        let range = range?;
        if candidate_id != self.candidate_id
            || range.start >= range.end
            || range.start < self.original_range.start
            || range.end > self.original_range.end
            || range.start % 2 != 0
            || range.end % 2 != 0
            || range.end.checked_sub(range.start)? != pcm.len() as u64
        {
            return None;
        }
        let start = usize::try_from(range.start.checked_sub(self.original_range.start)?).ok()?;
        let end = usize::try_from(range.end.checked_sub(self.original_range.start)?).ok()?;
        (self.original_pcm.get(start..end)? == pcm).then_some(self)
    }
}

impl SessionWakeVerificationSource {
    fn matches_input(
        &self,
        source_stream_id: u64,
        capture_generation: Option<u64>,
        segment_id: Option<u32>,
        metadata: Option<crate::embedded_audio::StreamingPcmChunkMetadata>,
    ) -> bool {
        metadata.is_some_and(|metadata| {
            source_stream_id == self.source_stream_id
                && capture_generation == Some(self.retained.capture.capture_generation)
                && segment_id == Some(self.retained.capture.segment_id)
                && metadata.collector_instance_id
                    == Some(self.retained.capture.collector_instance_id)
                && metadata.segment_ordinal == self.retained.capture.segment_ordinal
                && !metadata.metadata_incomplete
                && !metadata.revision_conflict
        })
    }
}

fn wake_pcm_digest(pcm: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(pcm).into()
}

impl WakeVerifierSnapshot {
    fn from_candidate(candidate: &BufferedSpeakerCandidate) -> Self {
        let range = candidate.current_candidate_range();
        let pcm: Arc<[u8]> = candidate.pcm.clone().into();
        Self {
            candidate_id: candidate.candidate_id,
            range,
            pcm_sha256: wake_pcm_digest(&pcm),
            pcm,
            source_facts: candidate.source_facts_for_range(range),
        }
    }

    fn observe_direct_result(
        self,
        result: &Result<crate::speaker_verification::VerificationResult, String>,
        candidate: &BufferedSpeakerCandidate,
    ) -> Option<RetainedWakeVerificationSource> {
        let observed_at = Instant::now();
        // Unknown or rewritten capture association cannot be repaired by a
        // later phrase/extraction acceptance or a different PCM snapshot.
        if self.source_facts != candidate.source_facts_for_range(self.range) {
            return None;
        }
        let range = candidate.current_candidate_range()?;
        let contexts = candidate.source_admission_contexts_for_range(Some(range));
        if contexts.is_empty()
            || contexts.iter().any(|(context, _, _)| {
                context.as_ref().map(|context| context.status(true))
                    != Some(CaptureAdmissionBindingStatus::MatchedConsumed)
            })
        {
            return None;
        }
        self.retain(
            result,
            candidate.candidate_id,
            &candidate.pcm,
            range,
            candidate.source_facts_for_range(Some(range)),
            observed_at,
        )
    }

    fn retain(
        self,
        result: &Result<crate::speaker_verification::VerificationResult, String>,
        current_candidate_id: u64,
        original_pcm: &[u8],
        original_range: crate::observability::CandidateRange,
        original_source_facts: Vec<crate::observability::CandidateSourceRunFact>,
        observed_at: Instant,
    ) -> Option<RetainedWakeVerificationSource> {
        let result = result.as_ref().ok()?;
        if !result.enrolled_owner_matched()
            || !result.matched
            || !result.score.is_finite()
            || current_candidate_id != self.candidate_id
            || original_pcm.is_empty()
            || original_pcm.len() > WAKE_DIAGNOSTIC_MAX_PCM_BYTES
            || original_range.end.checked_sub(original_range.start)? != original_pcm.len() as u64
        {
            return None;
        }
        let verified = self.range?;
        let capture = wake_capture_key(verified, &self.source_facts)?;
        if wake_capture_key(original_range, &original_source_facts)? != capture
            || verified.start < original_range.start
            || verified.end > original_range.end
            || verified.end.checked_sub(verified.start)? != self.pcm.len() as u64
            || wake_pcm_digest(&self.pcm) != self.pcm_sha256
        {
            return None;
        }
        let start = usize::try_from(verified.start.checked_sub(original_range.start)?).ok()?;
        let end = usize::try_from(verified.end.checked_sub(original_range.start)?).ok()?;
        if original_pcm.get(start..end)? != self.pcm.as_ref() {
            return None;
        }
        let original_pcm = if verified == original_range {
            self.pcm
        } else {
            Arc::from(original_pcm)
        };
        Some(RetainedWakeVerificationSource {
            candidate_id: self.candidate_id,
            capture,
            verifier_range: verified,
            verifier_pcm_sha256: self.pcm_sha256,
            original_range,
            original_pcm,
            original_source_facts,
            observed_at,
        })
    }
}

fn wake_capture_key(
    range: crate::observability::CandidateRange,
    facts: &[crate::observability::CandidateSourceRunFact],
) -> Option<WakeCaptureKey> {
    if range.start >= range.end || range.start % 2 != 0 || range.end % 2 != 0 {
        return None;
    }
    let mut cursor = range.start;
    let mut collector_cursor = None;
    let mut key = None;
    for fact in facts {
        let source = fact.candidate_range?;
        let emitted = fact.collector_emitted_range?;
        let metadata = fact.collector_metadata?;
        let current = WakeCaptureKey {
            capture_generation: fact.capture_generation?,
            segment_id: fact.segment_id?,
            collector_instance_id: metadata.collector_instance_id?,
            segment_ordinal: metadata.segment_ordinal,
        };
        if source.start != cursor
            || source.end <= source.start
            || source.end > range.end
            || source.end % 2 != 0
            || source.end.checked_sub(source.start)? != fact.bytes
            || emitted.end.checked_sub(emitted.start)? != fact.bytes
            || emitted.start % 2 != 0
            || emitted.end % 2 != 0
            || emitted.start < metadata.emitted_range.start
            || emitted.end > metadata.emitted_range.end
            || metadata.metadata_incomplete
            || metadata.revision_conflict
            || key.is_some_and(|key| key != current)
            || collector_cursor.is_some_and(|end| end != emitted.start)
        {
            return None;
        }
        key = Some(current);
        cursor = source.end;
        collector_cursor = Some(emitted.end);
    }
    (cursor == range.end).then_some(key).flatten()
}

impl EmbeddedAudioDictationSession {
    fn retire_wake_source_for_input(
        &mut self,
        capture_generation: Option<u64>,
        segment_id: Option<u32>,
        metadata: Option<crate::embedded_audio::StreamingPcmChunkMetadata>,
    ) {
        if self
            .retained_wake_verification_source
            .as_ref()
            .is_some_and(|source| {
                !source.matches_input(
                    self.source_stream_id,
                    capture_generation,
                    segment_id,
                    metadata,
                )
            })
        {
            self.retained_wake_verification_source = None;
            #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
            if let Some(asr) = self.volcengine_asr.as_ref() {
                asr.set_verified_capture_content_reference(None);
            }
        }
    }

    fn attach_retained_wake_verification_source(
        &mut self,
        source: Option<RetainedWakeVerificationSource>,
        provider_origin_candidate_byte: Option<u64>,
    ) {
        self.retained_wake_verification_source = source.and_then(|retained| {
            let origin = provider_origin_candidate_byte?;
            if origin < retained.original_range.start
                || origin > retained.original_range.end
                || origin % 2 != 0
            {
                return None;
            }
            Some(SessionWakeVerificationSource {
                source_stream_id: self.source_stream_id,
                provider_origin_candidate_byte: origin,
                retained,
            })
        });
        #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
        if let Some(asr) = self.volcengine_asr.as_ref() {
            let reference = self.retained_wake_verification_source.as_ref().and_then(|source| {
                let retained = &source.retained;
                let start = usize::try_from(retained.verifier_range.start.checked_sub(retained.original_range.start)?).ok()?;
                let end = usize::try_from(retained.verifier_range.end.checked_sub(retained.original_range.start)?).ok()?;
                let pcm = retained.original_pcm.get(start..end)?;
                (wake_pcm_digest(pcm) == retained.verifier_pcm_sha256).then(||pcm.to_vec())
            });
            asr.set_verified_capture_content_reference(reference);
        }
    }
}

#[cfg(test)]
#[path = "dictation_wake_verification_source_tests.rs"]
mod wake_verification_source_tests;

/// Complete local intent has a source range separate from an owner reference.
/// Near-match counters and voiceprint confidence cannot manufacture this proof.
#[derive(Clone, Copy, Debug)]
struct CompleteLocalWakeIntent {
    range: crate::observability::CandidateRange,
}

fn qualify_candidate_wake_intent(
    claimed: denzic_voice_activation_v1_core::PhraseSignal,
    keyword: crate::speech_decision_kernel::KeywordPhraseEvidence,
    local: Option<CompleteLocalWakeIntent>,
    current: Option<crate::observability::CandidateRange>,
    extracted_complete_phrase_and_owner: bool,
) -> denzic_voice_activation_v1_core::PhraseSignal {
    use denzic_voice_activation_v1_core::PhraseSignal;
    if keyword == crate::speech_decision_kernel::KeywordPhraseEvidence::Complete {
        return PhraseSignal::KeywordModel;
    }
    let local_covers_current_capture = local.zip(current).is_some_and(|(proof, current)| {
        proof.range.start >= current.start && proof.range.end > proof.range.start
            && proof.range.end <= current.end
    });
    if local_covers_current_capture
        || (extracted_complete_phrase_and_owner && claimed == PhraseSignal::LocalTranscript)
    { PhraseSignal::LocalTranscript } else { PhraseSignal::None }
}

fn retain_local_intent_proof(
    previous: Option<CompleteLocalWakeIntent>,
    matched: bool,
    relation: denzic_voice_activation_v1_core::LocalPhraseRelation,
    range: Option<crate::observability::CandidateRange>,
) -> Option<CompleteLocalWakeIntent> {
    if matched && crate::speech_decision_kernel::wake_phrase_signal(
        crate::speech_decision_kernel::KeywordPhraseEvidence::Unobserved,
        Some(relation),
    ) == denzic_voice_activation_v1_core::PhraseSignal::LocalTranscript {
        if let Some(range) = range.filter(|range| range.end > range.start) {
            return Some(CompleteLocalWakeIntent { range });
        }
    }
    previous
}

#[cfg(target_os = "windows")]
impl BufferedSpeakerCandidate {
    fn retain_complete_local_wake_intent(&mut self, result: &LocalWakeConfirmation) {
        self.complete_local_wake_intent = retain_local_intent_proof(
            self.complete_local_wake_intent, result.matched, result.phrase_relation,
            self.owner_local_phrase_source_range,
        );
    }
}
