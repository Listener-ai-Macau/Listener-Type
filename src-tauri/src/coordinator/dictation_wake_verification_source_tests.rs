use super::*;

fn fact(start: u64, end: u64) -> crate::observability::CandidateSourceRunFact {
    let emitted = crate::embedded_audio::StreamingPcmRange { start, end };
    crate::observability::CandidateSourceRunFact {
        capture_generation: Some(7),
        segment_id: Some(31),
        candidate_range: Some(crate::observability::CandidateRange { start, end }),
        collector_metadata: Some(crate::embedded_audio::StreamingPcmChunkMetadata {
            collector_instance_id: Some(11),
            segment_ordinal: 3,
            packet_sequence: (start / 8) as u16,
            emission_ordinal: start / 8,
            emitted_range: emitted,
            packet_revision: 0,
            packet_disposition: crate::embedded_audio::StreamingPcmChunkDisposition::New,
            wire_payload_bytes: (end - start) as usize,
            declared_pcm_bytes: (end - start) as usize,
            expanded_pcm_bytes: (end - start) as usize,
            previous_emission_ordinal: None,
            previous_emitted_range: None,
            revision_conflict: false,
            metadata_incomplete: false,
        }),
        collector_emitted_range: Some(emitted),
        bytes: end - start,
    }
}

fn snapshot() -> WakeVerifierSnapshot {
    let pcm: Arc<[u8]> = (0u8..8).collect::<Vec<_>>().into();
    WakeVerifierSnapshot {
        candidate_id: 13,
        range: Some(crate::observability::CandidateRange { start: 0, end: 8 }),
        pcm_sha256: wake_pcm_digest(&pcm),
        pcm,
        source_facts: vec![fact(0, 8)],
    }
}

fn matched() -> Result<crate::speaker_verification::VerificationResult, String> {
    Ok(crate::speaker_verification::VerificationResult {
        matched: true,
        owner_matched: true,
        score: 0.6,
        policy: crate::speaker_verification::VerificationPolicy::Enrolled,
    })
}

fn retain(
    input: WakeVerifierSnapshot,
    result: Result<crate::speaker_verification::VerificationResult, String>,
    candidate_id: u64,
    pcm: &[u8],
    facts: Vec<crate::observability::CandidateSourceRunFact>,
) -> Option<RetainedWakeVerificationSource> {
    input.retain(
        &result,
        candidate_id,
        pcm,
        crate::observability::CandidateRange {
            start: 0,
            end: pcm.len() as u64,
        },
        facts,
        Instant::now(),
    )
}

#[test]
fn exact_prefetched_wake_survives_later_provider_crop() {
    let input = snapshot();
    let digest = input.pcm_sha256;
    let mut candidate_pcm: Vec<u8> = (0..16).collect();
    let observed_at = Instant::now();
    let source = input
        .retain(
            &matched(),
            13,
            &candidate_pcm,
            crate::observability::CandidateRange { start: 0, end: 16 },
            vec![fact(0, 8), fact(8, 16)],
            observed_at,
        )
        .unwrap();
    candidate_pcm.drain(..4);
    assert_eq!(source.original_pcm.as_ref(), &(0u8..16).collect::<Vec<_>>());
    assert_eq!(source.verifier_pcm_sha256, digest);
    assert_eq!(source.verifier_range.end, 8);
    assert_eq!(source.original_range.end, 16);
    assert_eq!(source.observed_at, observed_at);
    assert_eq!(source.capture.segment_id, 31);
}

#[test]
fn open_policy_phrase_recovery_and_verifier_errors_supply_no_owner_receipt() {
    let pcm: Vec<u8> = (0..8).collect();
    for policy in [
        crate::speaker_verification::VerificationPolicy::OpenUnenrolled,
        crate::speaker_verification::VerificationPolicy::OpenInactiveProfile,
    ] {
        let mut result = matched().unwrap();
        result.policy = policy;
        assert!(retain(snapshot(), Ok(result), 13, &pcm, vec![fact(0, 8)]).is_none());
    }
    let mut result = matched().unwrap();
    result.owner_matched = false;
    assert!(retain(snapshot(), Ok(result), 13, &pcm, vec![fact(0, 8)]).is_none());
    assert!(retain(
        snapshot(),
        Err("verifier failed".into()),
        13,
        &pcm,
        vec![fact(0, 8)]
    )
    .is_none());
}

#[test]
fn stale_candidate_modified_bytes_future_frontier_and_changed_capture_are_pending() {
    let pcm: Vec<u8> = (0..8).collect();
    assert!(retain(snapshot(), matched(), 14, &pcm, vec![fact(0, 8)]).is_none());
    let mut changed = pcm.clone();
    changed[0] ^= 1;
    assert!(retain(snapshot(), matched(), 13, &changed, vec![fact(0, 8)]).is_none());
    assert!(retain(snapshot(), matched(), 13, &pcm[..6], vec![fact(0, 6)]).is_none());
    let mut changed = fact(0, 8);
    changed.capture_generation = Some(8);
    assert!(retain(snapshot(), matched(), 13, &pcm, vec![changed]).is_none());
}

#[test]
fn missing_conflicting_gapped_and_overlapping_capture_mapping_is_pending() {
    let range = crate::observability::CandidateRange { start: 0, end: 16 };
    assert!(wake_capture_key(range, &[fact(0, 8), fact(8, 16)]).is_some());
    for facts in [
        vec![fact(0, 6), fact(8, 16)],
        vec![fact(0, 10), fact(8, 16)],
        vec![fact(0, 8)],
    ] {
        assert!(wake_capture_key(range, &facts).is_none());
    }
    let mut missing = fact(0, 8);
    missing.collector_metadata = None;
    assert!(wake_capture_key(range, &[missing, fact(8, 16)]).is_none());
    let mut conflict = fact(8, 16);
    conflict
        .collector_metadata
        .as_mut()
        .unwrap()
        .revision_conflict = true;
    assert!(wake_capture_key(range, &[fact(0, 8), conflict]).is_none());
    let mut gap = fact(8, 16);
    gap.collector_emitted_range =
        Some(crate::embedded_audio::StreamingPcmRange { start: 10, end: 18 });
    assert!(wake_capture_key(range, &[fact(0, 8), gap]).is_none());
}

#[test]
fn oversized_or_unaligned_retention_is_pending() {
    let pcm = vec![0; WAKE_DIAGNOSTIC_MAX_PCM_BYTES + 2];
    assert!(retain(
        snapshot(),
        matched(),
        13,
        &pcm,
        vec![fact(0, pcm.len() as u64)]
    )
    .is_none());
    let range = crate::observability::CandidateRange { start: 1, end: 9 };
    assert!(wake_capture_key(range, &[fact(1, 9)]).is_none());
}

fn full_retained_source() -> RetainedWakeVerificationSource {
    let pcm: Vec<u8> = (0..16).collect();
    retain(
        snapshot(),
        matched(),
        13,
        &pcm,
        vec![fact(0, 8), fact(8, 16)],
    )
    .unwrap()
}

#[test]
fn terminal_body_keeps_only_its_exact_original_candidate_slice() {
    let range = Some(crate::observability::CandidateRange { start: 8, end: 16 });
    let body: Vec<u8> = (8..16).collect();
    let retained = full_retained_source()
        .for_terminal_body(13, range, &body)
        .unwrap();
    assert_eq!(retained.verifier_range.end, 8);
    assert_eq!(retained.original_range.end, 16);
    assert_eq!(
        retained.original_pcm.as_ref(),
        &(0u8..16).collect::<Vec<_>>()
    );
}

#[test]
fn wrong_candidate_changed_unknown_empty_or_future_body_gets_no_receipt() {
    let range = Some(crate::observability::CandidateRange { start: 8, end: 16 });
    let body: Vec<u8> = (8..16).collect();
    assert!(full_retained_source()
        .for_terminal_body(14, range, &body)
        .is_none());
    assert!(full_retained_source()
        .for_terminal_body(13, None, &body)
        .is_none());
    let mut changed = body.clone();
    changed[0] ^= 1;
    assert!(full_retained_source()
        .for_terminal_body(13, range, &changed)
        .is_none());
    let empty = Some(crate::observability::CandidateRange { start: 16, end: 16 });
    assert!(full_retained_source()
        .for_terminal_body(13, empty, &[])
        .is_none());
    let future = Some(crate::observability::CandidateRange { start: 16, end: 24 });
    assert!(full_retained_source()
        .for_terminal_body(13, future, &body)
        .is_none());
}

#[test]
fn same_capture_input_matches_and_new_or_unknown_capture_does_not() {
    let source = SessionWakeVerificationSource {
        source_stream_id: 41,
        provider_origin_candidate_byte: 8,
        retained: full_retained_source(),
    };
    let metadata = fact(16, 24).collector_metadata;
    assert!(source.matches_input(41, Some(7), Some(31), metadata));
    for (stream, generation, segment) in [
        (42, Some(7), Some(31)),
        (41, Some(8), Some(31)),
        (41, Some(7), Some(32)),
        (41, None, Some(31)),
        (41, Some(7), None),
    ] {
        assert!(!source.matches_input(stream, generation, segment, metadata));
    }
    assert!(!source.matches_input(41, Some(7), Some(31), None));
    let mut changed = metadata.unwrap();
    changed.collector_instance_id = Some(12);
    assert!(!source.matches_input(41, Some(7), Some(31), Some(changed)));
    changed = metadata.unwrap();
    changed.segment_ordinal += 1;
    assert!(!source.matches_input(41, Some(7), Some(31), Some(changed)));
    changed = metadata.unwrap();
    changed.revision_conflict = true;
    assert!(!source.matches_input(41, Some(7), Some(31), Some(changed)));
}
