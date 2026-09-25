use super::{
    acknowledge_automatic_wake_capsule_visible, append_typed_prefix,
    arm_accepted_automatic_wake_text_guard, arm_automatic_wake_text_guard,
    automatic_wake_body_started, automatic_wake_initial_body_wait_active,
    automatic_wake_session_active, begin_embedded_audio_dictation_session_id,
    begin_embedded_audio_preview_session, cancel_embedded_ble_listener_capture, cancel_session,
    claim_post_dictation_key, clear_automatic_wake_text_guard, clear_embedded_ble_cancel_flag,
    current_embedded_audio_final_preview_candidate, current_embedded_audio_partial_preview,
    default_done_message, device_ai_processing_completion_delay, device_ai_processing_io_allowed,
    device_processing_final_succeeded, dictation_asr_engine_backend_id,
    dictation_asr_quality_warning, dictation_asr_uses_core_accurate_engine, dictation_error_code,
    drive_polish_prefetch, embedded_audio_stop_feedback_latched,
    embedded_audio_stop_is_user_initiated, embedded_ble_listener_capture_ready,
    embedded_ble_processing_sync_disabled, embedded_ble_session_actor_history,
    embedded_ble_session_event_should_trace, embedded_ble_stream_idle_timeout,
    embedded_pcm_capsule_level, embedded_pcm_rms_and_peak, embedded_pcm_visual_level,
    embedded_streaming_chunk_is_asr_input, emit_embedded_audio_transcribing_if_active,
    end_embedded_ble_session, filter_automatic_wake_text, filter_dictation_visual_preview_text,
    finalize_polished_text, finish_asr_failure_with_history,
    finish_dictation_pipeline_error, finish_dictation_timeout,
    install_embedded_ble_listener_cancel, invalidate_embedded_audio_authoritative_preview,
    mark_automatic_wake_stop_requested, mark_embedded_ble_listener_ready,
    normalize_embedded_pcm_for_asr, normalize_embedded_streaming_pcm_for_asr,
    normalized_stage_portion, polish_prefetch_adoptable, preserve_recording_transcript,
    publish_embedded_ble_asr_final, record_embedded_ble_session_actor_command,
    record_pcm_stage_mapping_for_run, register_embedded_ble_cancel_flag,
    remove_standalone_dictation_fillers, request_embedded_audio_stop_feedback,
    request_embedded_ble_recording_stop_from_host, resolve_streaming_delivery_after_typer_drain,
    should_restore_clipboard_after_dictation, should_send_post_dictation_key,
    store_embedded_audio_stats, streaming_insert_eligible, update_embedded_audio_partial_preview,
    wayland_done_message, EmbeddedAudioDictationSession, EmbeddedBleSessionActorCommand,
    EmbeddedStreamingAgcState, EmbeddedStreamingDictation, DEVICE_AI_PROCESSING_MAX_VISIBLE_MS,
    DEVICE_AI_PROCESSING_MIN_VISIBLE_MS, EMBEDDED_AUDIO_FEED_CHUNK_BYTES,
    EMBEDDED_AUDIO_HOST_LIMITER_PEAK, EMBEDDED_BLE_DISABLE_PROCESSING_SYNC_ENV,
    EMBEDDED_STREAMING_PROACTIVE_STOP_SILENCE_MS, LOCAL_CONFIRMATION_START_BYTES,
    LOCAL_CONFIRMATION_START_MS, LocalSpeechActivity, PRESERVED_CANDIDATE_LEDGER_CAPACITY,
};
use crate::coordinator::Coordinator;
use crate::coordinator::{PolishPrefetch, PolishPrefetchBuf};
use crate::coordinator_state::{new_session_id, SessionPhase};
use crate::embedded_audio::{
    build_audio_data_notification, build_session_start_notification,
    build_session_stop_notification, StreamingPcmChunk, StreamingSessionEvent,
};
use crate::types::{
    ChineseScriptPreference, CorrectionRule, DictationInputSource, InsertStatus, PolishMode,
    PostDictationKey, UserPreferences,
};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Default)]
struct DeferredBridgeTestConsumer {
    pcm: Mutex<Vec<u8>>,
    sources: Mutex<Vec<Option<u32>>>,
    observations: Mutex<Vec<Option<u64>>>,
    intervals: Mutex<Vec<Option<crate::observability::PcmRange>>>,
}

#[test]
fn terminal_candidate_rejection_does_not_stop_the_next_unobserved_device_segment() {
    use super::{reject_hidden_automatic_candidate, HiddenCandidateTransportState};
    let coordinator = Coordinator::new();
    let inner = &coordinator.inner;
    assert!(inner.recording_lifecycle.lock().begin_candidate(100));
    // N has sent STOP. Local inference finishes only after the device started
    // N+1, whose START has not reached this host actor yet.
    assert!(!reject_hidden_automatic_candidate(
        inner,
        "wake_phrase_non_match",
        100,
        HiddenCandidateTransportState::Ended,
    ));
    assert!(!inner.recording_lifecycle.lock().hidden_candidate_active());
    assert!(inner.recording_lifecycle.lock().begin_candidate(101));
    // A delayed result for N cannot close or stop the now-observed N+1.
    assert!(!reject_hidden_automatic_candidate(
        inner,
        "wake_phrase_non_match",
        100,
        HiddenCandidateTransportState::Streaming,
    ));
    assert_eq!(
        inner
            .recording_lifecycle
            .lock()
            .current_candidate_session_id(),
        Some(101)
    );
    // A rejected live candidate must still stop ambient capture and re-arm.
    assert!(reject_hidden_automatic_candidate(
        inner,
        "wake_phrase_non_match",
        101,
        HiddenCandidateTransportState::Streaming,
    ));
    assert!(!inner.recording_lifecycle.lock().hidden_candidate_active());
}

impl crate::asr::AudioConsumer for DeferredBridgeTestConsumer {
    fn consume_pcm_chunk(&self, pcm: &[u8]) {
        self.pcm
            .lock()
            .expect("test pcm lock")
            .extend_from_slice(pcm);
    }

    fn consume_pcm_chunk_with_source(
        &self,
        pcm: &[u8],
        observation: Option<Arc<crate::observability::EmbeddedAudioPipelineObservation>>,
        segment_id: Option<u32>,
    ) {
        self.consume_pcm_chunk(pcm);
        self.sources
            .lock()
            .expect("test source lock")
            .push(segment_id);
        self.observations
            .lock()
            .expect("test observation lock")
            .push(observation.map(|item| item.capture_generation()));
    }

    fn consume_pcm_chunk_with_source_interval(
        &self,
        pcm: &[u8],
        observation: Option<Arc<crate::observability::EmbeddedAudioPipelineObservation>>,
        segment_id: Option<u32>,
        source_interval: Option<crate::observability::PcmSourceInterval>,
    ) {
        self.consume_pcm_chunk_with_source(pcm, observation, segment_id);
        self.intervals
            .lock()
            .expect("test interval lock")
            .push(source_interval.map(|interval| interval.range));
    }
}

impl crate::recorder::AudioConsumer for DeferredBridgeTestConsumer {
    fn consume_pcm_chunk(&self, pcm: &[u8]) {
        self.pcm
            .lock()
            .expect("test pcm lock")
            .extend_from_slice(pcm);
    }

    fn consume_pcm_chunk_with_source(
        &self,
        pcm: &[u8],
        observation: Option<Arc<crate::observability::EmbeddedAudioPipelineObservation>>,
        segment_id: Option<u32>,
    ) {
        self.consume_pcm_chunk(pcm);
        self.sources
            .lock()
            .expect("test source lock")
            .push(segment_id);
        self.observations
            .lock()
            .expect("test observation lock")
            .push(observation.map(|item| item.capture_generation()));
    }

    fn consume_pcm_chunk_with_source_interval(
        &self,
        pcm: &[u8],
        observation: Option<Arc<crate::observability::EmbeddedAudioPipelineObservation>>,
        segment_id: Option<u32>,
        source_interval: Option<crate::observability::PcmSourceInterval>,
    ) {
        self.consume_pcm_chunk_with_source(pcm, observation, segment_id);
        self.intervals
            .lock()
            .expect("test interval lock")
            .push(source_interval.map(|interval| interval.range));
    }
}

#[test]
fn deferred_asr_bridge_flushes_prefix_once_and_forwards_tail_in_order() {
    let bridge = super::DeferredAsrBridge::new();
    crate::recorder::AudioConsumer::consume_pcm_chunk(&bridge, &[1, 2, 3]);
    crate::recorder::AudioConsumer::consume_pcm_chunk(&bridge, &[4, 5]);

    let target = Arc::new(DeferredBridgeTestConsumer::default());
    let asr_target: Arc<dyn crate::asr::AudioConsumer> = target.clone();
    assert_eq!(bridge.attach(asr_target), 5);
    crate::recorder::AudioConsumer::consume_pcm_chunk(&bridge, &[6, 7]);

    assert_eq!(
        target.pcm.lock().expect("test pcm lock").as_slice(),
        &[1, 2, 3, 4, 5, 6, 7]
    );
}

#[test]
fn deferred_asr_bridge_preserves_full_wake_body_prefix_during_attach() {
    // Reproduce the 2.6 s connection prefix whose old 300 ms cap deleted
    // 2.3 s. Add live audio during attachment to exercise the hand-off too.
    struct ReentrantConsumer {
        bridge: std::sync::Weak<super::DeferredAsrBridge>,
        pcm: Mutex<Vec<u8>>,
        appended: AtomicBool,
    }
    impl crate::asr::AudioConsumer for ReentrantConsumer {
        fn consume_pcm_chunk(&self, pcm: &[u8]) {
            self.pcm.lock().unwrap().extend_from_slice(pcm);
            if !self.appended.swap(true, Ordering::SeqCst) {
                crate::recorder::AudioConsumer::consume_pcm_chunk(
                    self.bridge.upgrade().unwrap().as_ref(),
                    &[91, 92, 93, 94],
                );
            }
        }
    }
    let bridge = Arc::new(super::DeferredAsrBridge::new());
    let prefix: Vec<u8> = (0..83_200).map(|i| (i % 251) as u8).collect();
    for chunk in prefix.chunks(3_200) {
        crate::recorder::AudioConsumer::consume_pcm_chunk(bridge.as_ref(), chunk);
    }
    let target = Arc::new(ReentrantConsumer {
        bridge: Arc::downgrade(&bridge),
        pcm: Mutex::new(Vec::new()),
        appended: AtomicBool::new(false),
    });
    assert_eq!(bridge.attach(target.clone()), prefix.len() + 4);
    crate::recorder::AudioConsumer::consume_pcm_chunk(bridge.as_ref(), &[95, 96]);
    let mut expected = prefix;
    expected.extend_from_slice(&[91, 92, 93, 94, 95, 96]);
    assert_eq!(*target.pcm.lock().unwrap(), expected);
}

#[test]
fn deferred_asr_bridge_preserves_carried_segment_source_during_attach() {
    let bridge = super::DeferredAsrBridge::new();
    let guard = crate::observability::begin_embedded_audio_pipeline_capture(301);
    let observation = guard.observation();
    crate::recorder::AudioConsumer::consume_pcm_chunk_with_source(
        &bridge,
        &[1, 2, 3, 4],
        Some(Arc::clone(&observation)),
        Some(41),
    );
    drop(guard);

    let target = Arc::new(DeferredBridgeTestConsumer::default());
    let asr_target: Arc<dyn crate::asr::AudioConsumer> = target.clone();
    assert_eq!(bridge.attach(asr_target), 4);
    assert_eq!(&*target.pcm.lock().expect("test pcm lock"), &[1, 2, 3, 4]);
    assert_eq!(
        &*target.sources.lock().expect("test source lock"),
        &[Some(41)]
    );
}

#[test]
fn deferred_asr_bridge_carries_explicit_source_interval_during_attach() {
    let bridge = super::DeferredAsrBridge::new();
    let guard = crate::observability::begin_embedded_audio_pipeline_capture(303);
    let observation = guard.observation();
    let interval = observation
        .allocate_source_interval(1_242, Some(42), 4)
        .expect("source interval");
    let delayed_interval = observation
        .allocate_source_interval(1_242, Some(42), 4)
        .expect("delayed source interval");
    crate::recorder::AudioConsumer::consume_pcm_chunk_with_source_interval(
        &bridge,
        &[1, 2, 3, 4],
        Some(Arc::clone(&observation)),
        Some(42),
        Some(interval),
    );
    crate::recorder::AudioConsumer::consume_pcm_chunk_with_source_interval(
        &bridge,
        &[5, 6, 7, 8],
        Some(Arc::clone(&observation)),
        Some(42),
        Some(delayed_interval),
    );

    let target = Arc::new(DeferredBridgeTestConsumer::default());
    let asr_target: Arc<dyn crate::asr::AudioConsumer> = target.clone();
    assert_eq!(bridge.attach(asr_target), 8);
    assert_eq!(
        *target.intervals.lock().expect("test interval lock"),
        vec![
            Some(crate::observability::PcmRange { start: 0, end: 4 }),
            Some(crate::observability::PcmRange { start: 4, end: 8 }),
        ]
    );
}

#[test]
fn streaming_source_run_split_preserves_interval_remainder_at_frame_boundary() {
    let guard = crate::observability::begin_embedded_audio_pipeline_capture(304);
    let observation = guard.observation();
    let interval = observation
        .allocate_source_interval(1_243, Some(43), 5_000)
        .expect("source interval");
    let collector_metadata = crate::embedded_audio::StreamingPcmChunkMetadata {
        collector_instance_id: Some(17),
        segment_ordinal: 1,
        packet_sequence: 4,
        emission_ordinal: 0,
        emitted_range: crate::embedded_audio::StreamingPcmRange {
            start: 0,
            end: 5_000,
        },
        packet_revision: 0,
        packet_disposition: crate::embedded_audio::StreamingPcmChunkDisposition::New,
        wire_payload_bytes: 5_000,
        declared_pcm_bytes: 5_000,
        expanded_pcm_bytes: 5_000,
        previous_emission_ordinal: None,
        previous_emitted_range: None,
        revision_conflict: false,
        metadata_incomplete: false,
    };
    let mut source_runs = vec![super::EmbeddedStreamingPcmSourceRun {
        bytes: 5_000,
        observation: Some(Arc::clone(&observation)),
        segment_id: Some(43),
        source_interval: Some(interval),
        collector_metadata: Some(collector_metadata),
        collector_emitted_range: Some(collector_metadata.emitted_range),
    }];

    assert!(super::take_streaming_pcm_source_runs(&mut source_runs, 0).is_empty());
    let first = super::take_streaming_pcm_source_runs(&mut source_runs, 3_200);
    let second = super::take_streaming_pcm_source_runs(&mut source_runs, 1_800);
    assert_eq!(first.len(), 1);
    assert_eq!(second.len(), 1);
    assert_eq!(first[0].bytes, 3_200);
    assert_eq!(second[0].bytes, 1_800);
    assert_eq!(
        first[0].source_interval.map(|interval| interval.range),
        Some(crate::observability::PcmRange {
            start: 0,
            end: 3_200
        })
    );
    assert_eq!(
        second[0].source_interval.map(|interval| interval.range),
        Some(crate::observability::PcmRange {
            start: 3_200,
            end: 5_000
        })
    );
    assert_eq!(
        first[0].collector_emitted_range,
        Some(crate::embedded_audio::StreamingPcmRange {
            start: 0,
            end: 3_200
        })
    );
    assert_eq!(
        second[0].collector_emitted_range,
        Some(crate::embedded_audio::StreamingPcmRange {
            start: 3_200,
            end: 5_000
        })
    );
    assert!(source_runs.is_empty());
}

#[test]
fn candidate_collector_range_is_clipped_when_releasing_a_middle_slice() {
    let full_candidate = crate::observability::CandidateRange {
        start: 0,
        end: 5_000,
    };
    let selected_candidate = crate::observability::CandidateRange {
        start: 1_200,
        end: 3_200,
    };
    let full_emitted = crate::embedded_audio::StreamingPcmRange {
        start: 100,
        end: 5_100,
    };

    assert_eq!(
        super::collector_emitted_range_for_candidate_overlap(
            Some(full_emitted),
            Some(full_candidate),
            Some(selected_candidate),
        ),
        Some(crate::embedded_audio::StreamingPcmRange {
            start: 1_300,
            end: 3_300,
        })
    );
    assert_eq!(
        super::collector_emitted_range_for_candidate_overlap(
            Some(full_emitted),
            Some(full_candidate),
            None,
        ),
        Some(full_emitted)
    );
}

#[test]
fn production_source_drain_keeps_adjacent_collector_emissions_separate() {
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    {
        let mut state = coordinator.inner.state.lock();
        state.session_id = session_id;
        state.phase = SessionPhase::Listening;
        state.cancelled = false;
    }
    let consumer = Arc::new(CapturingConsumer::default());
    let consumer_for_session: Arc<dyn crate::recorder::AudioConsumer> = consumer;
    let mut session = embedded_audio_test_session(session_id, consumer_for_session);
    session.active_asr = "volcengine".into();
    let guard = crate::observability::begin_embedded_audio_pipeline_capture(319);
    let observation = guard.observation();
    session.attach_pipeline_observation(Some(Arc::clone(&observation)));
    let metadata =
        |emission_ordinal, start, end| crate::embedded_audio::StreamingPcmChunkMetadata {
            collector_instance_id: Some(18),
            segment_ordinal: 1,
            packet_sequence: 4,
            emission_ordinal,
            emitted_range: crate::embedded_audio::StreamingPcmRange { start, end },
            packet_revision: 0,
            packet_disposition: crate::embedded_audio::StreamingPcmChunkDisposition::New,
            wire_payload_bytes: (end - start) as usize,
            declared_pcm_bytes: (end - start) as usize,
            expanded_pcm_bytes: (end - start) as usize,
            previous_emission_ordinal: None,
            previous_emitted_range: None,
            revision_conflict: false,
            metadata_incomplete: false,
        };

    session
        .consume_streaming_pcm_from_segment_with_collector_metadata(
            &coordinator.inner,
            &vec![1_u8; 3_200],
            None,
            Some(92),
            Some(Arc::clone(&observation)),
            true,
            Some(metadata(0, 0, 3_200)),
        )
        .expect("first collector emission");
    session
        .consume_streaming_pcm_from_segment_with_collector_metadata(
            &coordinator.inner,
            &vec![2_u8; 1_800],
            None,
            Some(92),
            Some(Arc::clone(&observation)),
            true,
            Some(metadata(1, 3_200, 5_000)),
        )
        .expect("second collector emission");
    session.flush_streaming_pcm();

    let normalized = observation
        .pcm_stage_facts_for_test()
        .into_iter()
        .filter(|fact| fact.stage == crate::observability::PcmStage::Normalized)
        .collect::<Vec<_>>();
    assert_eq!(normalized.len(), 2);
    assert_eq!(normalized[0].source_shares.len(), 1);
    assert_eq!(normalized[1].source_shares.len(), 1);
    assert_eq!(
        normalized[0].source_shares[0].collector_emitted_range,
        Some(crate::embedded_audio::StreamingPcmRange {
            start: 0,
            end: 3_200
        })
    );
    assert_eq!(
        normalized[1].source_shares[0].collector_emitted_range,
        Some(crate::embedded_audio::StreamingPcmRange {
            start: 3_200,
            end: 5_000
        })
    );
}

#[test]
fn coordinator_stage_mapping_tracks_accept_archive_normalized_and_flush_ranges() {
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    {
        let mut state = coordinator.inner.state.lock();
        state.session_id = session_id;
        state.phase = SessionPhase::Listening;
        state.cancelled = false;
    }
    let consumer = Arc::new(CapturingConsumer::default());
    let consumer_for_session: Arc<dyn crate::recorder::AudioConsumer> = consumer.clone();
    let mut session = embedded_audio_test_session(session_id, consumer_for_session.clone());
    session.active_asr = "volcengine".into();
    let guard = crate::observability::begin_embedded_audio_pipeline_capture(305);
    let observation = guard.observation();
    session.attach_pipeline_observation(Some(Arc::clone(&observation)));

    let first = vec![1_u8; EMBEDDED_AUDIO_FEED_CHUNK_BYTES];
    let second = vec![2_u8; 1_800];
    session
        .consume_streaming_pcm(&coordinator.inner, &first, None)
        .expect("first block accepted");
    session
        .consume_streaming_pcm(&coordinator.inner, &second, None)
        .expect("second block accepted");
    session.flush_streaming_pcm();

    let facts = observation.pcm_stage_facts_for_test();
    assert_eq!(facts.len(), 6);
    for fact in &facts {
        assert_eq!(fact.logical_stream_id, session.source_stream_id);
        assert_eq!(
            fact.stream_kind,
            crate::observability::PcmStreamKind::CoordinatorInputPcm
        );
        assert_eq!(
            fact.pcm_format,
            crate::observability::PcmFormat::PcmS16LeMono16k
        );
        assert_eq!(
            fact.mapping,
            crate::observability::PcmMappingKind::PositionPreserving
        );
        assert_eq!(
            fact.disposition,
            crate::observability::PcmStageDisposition::Appended
        );
        assert_eq!(fact.source_shares.len(), 1);
        assert_eq!(
            fact.source_shares[0].destination_range,
            fact.destination_range
        );
        assert_eq!(
            fact.source_shares[0]
                .source_interval
                .expect("source interval")
                .range,
            fact.destination_range.expect("destination range")
        );
    }
    for stage in [
        crate::observability::PcmStage::CoordinatorAccepted,
        crate::observability::PcmStage::Archive,
        crate::observability::PcmStage::Normalized,
    ] {
        let ranges = facts
            .iter()
            .filter(|fact| fact.stage == stage)
            .map(|fact| fact.destination_range.expect("stage range"))
            .collect::<Vec<_>>();
        assert_eq!(
            ranges,
            vec![
                crate::observability::PcmRange {
                    start: 0,
                    end: EMBEDDED_AUDIO_FEED_CHUNK_BYTES as u64,
                },
                crate::observability::PcmRange {
                    start: EMBEDDED_AUDIO_FEED_CHUNK_BYTES as u64,
                    end: 5_000,
                },
            ],
            "stage={stage:?}"
        );
    }
}

#[test]
fn coordinator_stage_mapping_marks_archive_disabled_without_faking_a_range() {
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    {
        let mut state = coordinator.inner.state.lock();
        state.session_id = session_id;
        state.phase = SessionPhase::Listening;
        state.cancelled = false;
    }
    let consumer = Arc::new(CapturingConsumer::default());
    let consumer_for_session: Arc<dyn crate::recorder::AudioConsumer> = consumer;
    let mut session = embedded_audio_test_session(session_id, consumer_for_session);
    session.active_asr = "volcengine".into();
    session.archive_pcm = None;
    let guard = crate::observability::begin_embedded_audio_pipeline_capture(306);
    let observation = guard.observation();
    session.attach_pipeline_observation(Some(Arc::clone(&observation)));
    let pcm = vec![3_u8; EMBEDDED_AUDIO_FEED_CHUNK_BYTES];

    session
        .consume_streaming_pcm(&coordinator.inner, &pcm, None)
        .expect("accepted block");
    let archive = observation
        .pcm_stage_facts_for_test()
        .into_iter()
        .find(|fact| fact.stage == crate::observability::PcmStage::Archive)
        .expect("archive stage fact");
    assert_eq!(
        archive.disposition,
        crate::observability::PcmStageDisposition::Disabled
    );
    assert_eq!(archive.destination_range, None);
    assert_eq!(archive.source_shares[0].destination_range, None);
    assert_eq!(archive.source_shares[0].bytes, pcm.len() as u64);
    assert!(observation
        .pcm_stage_facts_for_test()
        .iter()
        .filter(|fact| fact.stage == crate::observability::PcmStage::Normalized)
        .all(|fact| fact.destination_range.is_some()));
}

#[test]
fn coordinator_stage_mapping_keeps_mixed_source_identity_and_local_offsets() {
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    {
        let mut state = coordinator.inner.state.lock();
        state.session_id = session_id;
        state.phase = SessionPhase::Listening;
        state.cancelled = false;
    }
    let consumer = Arc::new(CapturingConsumer::default());
    let consumer_for_session: Arc<dyn crate::recorder::AudioConsumer> = consumer;
    let mut session = embedded_audio_test_session(session_id, consumer_for_session);
    session.active_asr = "volcengine".into();
    let first_guard = crate::observability::begin_embedded_audio_pipeline_capture(307);
    let first_observation = first_guard.observation();
    let second_guard = crate::observability::begin_embedded_audio_pipeline_capture(308);
    let second_observation = second_guard.observation();

    session
        .consume_streaming_pcm_from_segment_with_observation(
            &coordinator.inner,
            &vec![4_u8; 2_000],
            None,
            Some(51),
            Some(Arc::clone(&first_observation)),
            true,
        )
        .expect("first source block");
    session
        .consume_streaming_pcm_from_segment_with_observation(
            &coordinator.inner,
            &vec![5_u8; 1_800],
            None,
            Some(52),
            Some(Arc::clone(&second_observation)),
            true,
        )
        .expect("second source block");
    session.flush_streaming_pcm();

    let first_facts = first_observation.pcm_stage_facts_for_test();
    let second_facts = second_observation.pcm_stage_facts_for_test();
    assert!(first_facts
        .iter()
        .all(|fact| fact.logical_stream_id == session.source_stream_id));
    assert!(second_facts
        .iter()
        .all(|fact| fact.logical_stream_id == session.source_stream_id));
    let first_normalized = first_facts
        .iter()
        .find(|fact| fact.stage == crate::observability::PcmStage::Normalized)
        .expect("first normalized share");
    assert_eq!(
        first_normalized.destination_range,
        Some(crate::observability::PcmRange {
            start: 0,
            end: 2_000
        })
    );
    assert_eq!(
        first_normalized.source_shares[0]
            .source_interval
            .expect("first source interval")
            .range,
        crate::observability::PcmRange {
            start: 0,
            end: 2_000
        }
    );
    let second_normalized = second_facts
        .iter()
        .filter(|fact| fact.stage == crate::observability::PcmStage::Normalized)
        .collect::<Vec<_>>();
    assert_eq!(second_normalized.len(), 2);
    assert_eq!(
        second_normalized[0].destination_range,
        Some(crate::observability::PcmRange {
            start: 2_000,
            end: 3_200
        })
    );
    assert_eq!(
        second_normalized[0].source_shares[0]
            .source_interval
            .expect("second source interval prefix")
            .range,
        crate::observability::PcmRange {
            start: 0,
            end: 1_200
        }
    );
    assert_eq!(
        second_normalized[1].destination_range,
        Some(crate::observability::PcmRange {
            start: 3_200,
            end: 3_800
        })
    );
    assert_eq!(
        second_normalized[1].source_shares[0]
            .source_interval
            .expect("second source interval tail")
            .range,
        crate::observability::PcmRange {
            start: 1_200,
            end: 1_800
        }
    );
}

#[test]
fn normalized_stage_mapping_never_guesses_length_changing_coordinates() {
    let preserved = normalized_stage_portion(Some(100), 3_200, 3_200, 0, 3_200);
    assert_eq!(preserved.output_bytes, 3_200);
    assert_eq!(
        preserved.destination_range,
        Some(crate::observability::PcmRange {
            start: 100,
            end: 3_300
        })
    );
    assert_eq!(
        preserved.mapping,
        crate::observability::PcmMappingKind::PositionPreserving
    );

    let shortened = normalized_stage_portion(Some(0), 3_200, 1_600, 0, 3_200);
    assert_eq!(shortened.output_bytes, 1_600);
    assert_eq!(
        shortened.destination_range,
        Some(crate::observability::PcmRange {
            start: 0,
            end: 1_600
        })
    );
    assert_eq!(
        shortened.mapping,
        crate::observability::PcmMappingKind::Unknown
    );

    let lengthened = normalized_stage_portion(Some(0), 3_200, 4_800, 0, 3_200);
    assert_eq!(lengthened.output_bytes, 3_200);
    assert_eq!(
        lengthened.destination_range,
        Some(crate::observability::PcmRange {
            start: 0,
            end: 3_200
        })
    );
    assert_eq!(
        lengthened.mapping,
        crate::observability::PcmMappingKind::Unknown
    );

    let empty = normalized_stage_portion(Some(0), 3_200, 0, 0, 3_200);
    assert_eq!(empty.output_bytes, 0);
    assert_eq!(empty.destination_range, None);
    assert_eq!(empty.mapping, crate::observability::PcmMappingKind::Unknown);

    let guard = crate::observability::begin_embedded_audio_pipeline_capture(309);
    let observation = guard.observation();
    record_pcm_stage_mapping_for_run(
        Some(&observation),
        9_309,
        crate::observability::PcmStage::Normalized,
        crate::observability::PcmStageDisposition::Appended,
        shortened.mapping,
        shortened.destination_range,
        Some(61),
        None,
        shortened.output_bytes,
    );
    record_pcm_stage_mapping_for_run(
        Some(&observation),
        9_309,
        crate::observability::PcmStage::Normalized,
        crate::observability::PcmStageDisposition::Unknown,
        lengthened.mapping,
        lengthened.destination_range,
        Some(61),
        None,
        lengthened.output_bytes,
    );
    record_pcm_stage_mapping_for_run(
        Some(&observation),
        9_309,
        crate::observability::PcmStage::Normalized,
        crate::observability::PcmStageDisposition::Empty,
        empty.mapping,
        empty.destination_range,
        Some(61),
        None,
        3_200,
    );
    let facts = observation.pcm_stage_facts_for_test();
    assert_eq!(facts.len(), 3);
    assert!(facts
        .iter()
        .all(|fact| fact.mapping == crate::observability::PcmMappingKind::Unknown));
    assert_eq!(
        facts[2].disposition,
        crate::observability::PcmStageDisposition::Empty
    );
    assert!(observation.pcm_stage_capacity_drops_for_test().4);
}

#[test]
fn normalized_stage_production_loop_keeps_lengthened_output_unattributed() {
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    {
        let mut state = coordinator.inner.state.lock();
        state.session_id = session_id;
        state.phase = SessionPhase::Listening;
        state.cancelled = false;
    }
    let consumer = Arc::new(DeferredBridgeTestConsumer::default());
    let consumer_for_session: Arc<dyn crate::recorder::AudioConsumer> = consumer.clone();
    let mut session = embedded_audio_test_session(session_id, consumer_for_session.clone());
    session.active_asr = "volcengine".into();
    let guard = crate::observability::begin_embedded_audio_pipeline_capture(310);
    let observation = guard.observation();
    session.attach_pipeline_observation(Some(Arc::clone(&observation)));
    let source_interval = observation
        .allocate_source_interval(session.source_stream_id, Some(71), 3_200)
        .expect("source interval");
    let source_run = super::EmbeddedStreamingPcmSourceRun {
        bytes: 3_200,
        observation: Some(Arc::clone(&observation)),
        segment_id: Some(71),
        source_interval: Some(source_interval),
        collector_metadata: None,
        collector_emitted_range: None,
    };
    let normalized = vec![8_u8; 4_800];
    session.consume_normalized_pcm_with_source_runs(
        3_200,
        &normalized,
        &[source_run],
        Some(Arc::clone(&observation)),
        crate::observability::PcmMappingKind::PositionPreserving,
    );

    let normalized_projection_facts = observation
        .pcm_stage_facts_for_test()
        .into_iter()
        .filter(|fact| fact.stage == crate::observability::PcmStage::Normalized)
        .collect::<Vec<_>>();
    assert_eq!(normalized_projection_facts.len(), 1);
    assert!(normalized_projection_facts.iter().all(|fact| {
        fact.mapping == crate::observability::PcmMappingKind::Unknown
            && fact.source_shares[0].destination_range.is_none()
            && fact.source_shares[0].source_interval.is_some()
    }));
    let normalized_facts = session
        .pcm_stage_ledger
        .facts()
        .into_iter()
        .filter(|fact| fact.stage == crate::observability::PcmStage::Normalized)
        .collect::<Vec<_>>();
    assert_eq!(normalized_facts.len(), 1);
    assert!(normalized_facts.iter().all(|fact| {
        fact.mapping == crate::observability::PcmMappingKind::Unknown
            && fact.operation_id.is_some()
            && fact.source_shares.len() == 1
            && fact.source_shares[0].source_interval.is_some()
    }));
    assert_eq!(
        normalized_facts[0].destination_range,
        Some(crate::observability::PcmRange {
            start: 0,
            end: 4_800,
        })
    );
    assert!(normalized_facts.iter().any(|fact| {
        fact.destination_range
            == Some(crate::observability::PcmRange {
                start: 0,
                end: 4_800,
            })
    }));
    assert_eq!(
        consumer.pcm.lock().expect("pcm lock").len(),
        normalized.len()
    );
    assert!(observation.pcm_stage_capacity_drops_for_test().4);
}

#[test]
fn normalized_stage_production_loop_keeps_shortened_sources_known_but_unattributed() {
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    {
        let mut state = coordinator.inner.state.lock();
        state.session_id = session_id;
        state.phase = SessionPhase::Listening;
        state.cancelled = false;
    }
    let consumer = Arc::new(DeferredBridgeTestConsumer::default());
    let consumer_for_session: Arc<dyn crate::recorder::AudioConsumer> = consumer.clone();
    let mut session = embedded_audio_test_session(session_id, consumer_for_session);
    session.active_asr = "volcengine".into();
    let first_guard = crate::observability::begin_embedded_audio_pipeline_capture(311);
    let first_observation = first_guard.observation();
    let second_guard = crate::observability::begin_embedded_audio_pipeline_capture(312);
    let second_observation = second_guard.observation();
    let first_interval = first_observation
        .allocate_source_interval(session.source_stream_id, Some(81), 2_000)
        .expect("first interval");
    let second_interval = second_observation
        .allocate_source_interval(session.source_stream_id, Some(82), 3_000)
        .expect("second interval");
    let source_runs = [
        super::EmbeddedStreamingPcmSourceRun {
            bytes: 2_000,
            observation: Some(Arc::clone(&first_observation)),
            segment_id: Some(81),
            source_interval: Some(first_interval),
            collector_metadata: None,
            collector_emitted_range: None,
        },
        super::EmbeddedStreamingPcmSourceRun {
            bytes: 3_000,
            observation: Some(Arc::clone(&second_observation)),
            segment_id: Some(82),
            source_interval: Some(second_interval),
            collector_metadata: None,
            collector_emitted_range: None,
        },
    ];
    let normalized = vec![9_u8; 3_000];
    session.consume_normalized_pcm_with_source_runs(
        5_000,
        &normalized,
        &source_runs,
        None,
        crate::observability::PcmMappingKind::PositionPreserving,
    );

    let normalized_facts = session
        .pcm_stage_ledger
        .facts()
        .into_iter()
        .filter(|fact| fact.stage == crate::observability::PcmStage::Normalized)
        .collect::<Vec<_>>();
    assert_eq!(normalized_facts.len(), 1);
    assert_eq!(normalized_facts[0].operation_id, Some(1));
    assert_eq!(normalized_facts[0].source_shares.len(), 2);
    assert_eq!(
        normalized_facts[0].destination_range,
        Some(crate::observability::PcmRange {
            start: 0,
            end: 3_000,
        })
    );

    for observation in [&first_observation, &second_observation] {
        let facts = observation
            .pcm_stage_facts_for_test()
            .into_iter()
            .filter(|fact| fact.stage == crate::observability::PcmStage::Normalized)
            .collect::<Vec<_>>();
        assert!(!facts.is_empty());
        assert!(facts.iter().all(|fact| {
            fact.mapping == crate::observability::PcmMappingKind::Unknown
                && fact.destination_range
                    == Some(crate::observability::PcmRange {
                        start: 0,
                        end: 3_000,
                    })
                && fact.source_shares[0].destination_range.is_none()
                && fact.source_shares[0].source_interval.is_some()
        }));
        assert!(observation.pcm_stage_capacity_drops_for_test().4);
    }
    assert_eq!(
        consumer.pcm.lock().expect("pcm lock").len(),
        normalized.len()
    );
    assert_eq!(consumer.intervals.lock().expect("interval lock").len(), 2);
    assert!(consumer
        .intervals
        .lock()
        .expect("interval lock")
        .iter()
        .all(Option::is_none));
}

#[test]
fn normalized_stage_production_loop_records_zero_output_and_missing_source() {
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    {
        let mut state = coordinator.inner.state.lock();
        state.session_id = session_id;
        state.phase = SessionPhase::Listening;
        state.cancelled = false;
    }
    let consumer = Arc::new(DeferredBridgeTestConsumer::default());
    let consumer_for_session: Arc<dyn crate::recorder::AudioConsumer> = consumer.clone();
    let mut session = embedded_audio_test_session(session_id, consumer_for_session.clone());
    session.active_asr = "volcengine".into();
    let guard = crate::observability::begin_embedded_audio_pipeline_capture(313);
    let observation = guard.observation();
    session.attach_pipeline_observation(Some(Arc::clone(&observation)));
    let source_interval = observation
        .allocate_source_interval(session.source_stream_id, Some(91), 3_200)
        .expect("source interval");
    let source_run = super::EmbeddedStreamingPcmSourceRun {
        bytes: 3_200,
        observation: Some(Arc::clone(&observation)),
        segment_id: Some(91),
        source_interval: Some(source_interval),
        collector_metadata: None,
        collector_emitted_range: None,
    };
    session.consume_normalized_pcm_with_source_runs(
        3_200,
        &[],
        &[source_run],
        Some(Arc::clone(&observation)),
        crate::observability::PcmMappingKind::PositionPreserving,
    );
    let empty_fact = observation
        .pcm_stage_facts_for_test()
        .into_iter()
        .find(|fact| fact.stage == crate::observability::PcmStage::Normalized)
        .expect("zero-output fact");
    assert_eq!(
        empty_fact.disposition,
        crate::observability::PcmStageDisposition::Empty
    );
    assert_eq!(empty_fact.destination_range, None);
    assert_eq!(
        empty_fact.source_shares[0].source_interval,
        Some(source_interval)
    );
    assert!(observation.pcm_stage_capacity_drops_for_test().4);
    assert_eq!(
        session
            .pcm_stage_ledger
            .facts()
            .into_iter()
            .filter(|fact| fact.stage == crate::observability::PcmStage::Normalized)
            .count(),
        1
    );

    let missing_guard = crate::observability::begin_embedded_audio_pipeline_capture(314);
    let missing_observation = missing_guard.observation();
    let mut missing_session = embedded_audio_test_session(session_id, consumer_for_session);
    missing_session.active_asr = "volcengine".into();
    missing_session.consume_normalized_pcm_with_source_runs(
        3_200,
        &vec![7_u8; 3_200],
        &[],
        None,
        crate::observability::PcmMappingKind::Unknown,
    );
    let missing_fact = missing_session
        .pcm_stage_ledger
        .facts()
        .into_iter()
        .find(|fact| fact.stage == crate::observability::PcmStage::Normalized)
        .expect("missing-source fact");
    assert_eq!(
        missing_fact.mapping,
        crate::observability::PcmMappingKind::Unknown
    );
    assert_eq!(
        missing_fact.destination_range,
        Some(crate::observability::PcmRange {
            start: 0,
            end: 3_200,
        })
    );
    assert_eq!(missing_fact.source_shares[0].segment_id, None);
    assert_eq!(missing_fact.source_shares[0].source_interval, None);
    assert_eq!(missing_fact.source_shares[0].destination_range, None);
    assert!(missing_session.pcm_stage_ledger.capacity_drops().4);
    assert!(missing_observation.pcm_stage_facts_for_test().is_empty());
}

#[test]
fn session_stage_ledger_survives_observation_rebind_without_migrating_old_facts() {
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    {
        let mut state = coordinator.inner.state.lock();
        state.session_id = session_id;
        state.phase = SessionPhase::Listening;
        state.cancelled = false;
    }
    let consumer: Arc<dyn crate::recorder::AudioConsumer> = Arc::new(CapturingConsumer::default());
    let mut session = embedded_audio_test_session(session_id, consumer);
    session.active_asr = "volcengine".into();
    let old_guard = crate::observability::begin_embedded_audio_pipeline_capture(317);
    let old_observation = old_guard.observation();
    session.attach_pipeline_observation(Some(Arc::clone(&old_observation)));
    session
        .consume_streaming_pcm(&coordinator.inner, &vec![1_u8; 3_200], None)
        .expect("old observation block");

    let new_guard = crate::observability::begin_embedded_audio_pipeline_capture(318);
    let new_observation = new_guard.observation();
    session.attach_pipeline_observation(Some(Arc::clone(&new_observation)));
    session
        .consume_streaming_pcm(&coordinator.inner, &vec![2_u8; 3_200], None)
        .expect("new observation block");

    let old_ids = old_observation
        .pcm_stage_facts_for_test()
        .into_iter()
        .map(|fact| fact.operation_id)
        .collect::<Vec<_>>();
    let new_ids = new_observation
        .pcm_stage_facts_for_test()
        .into_iter()
        .map(|fact| fact.operation_id)
        .collect::<Vec<_>>();
    assert_eq!(old_ids.len(), 3);
    assert_eq!(new_ids.len(), 3);
    assert!(old_ids.iter().all(|id| *id == Some(1) || *id == Some(2)));
    assert!(new_ids.iter().all(|id| *id == Some(3) || *id == Some(4)));
    assert!(old_ids.iter().all(|id| !new_ids.contains(id)));
    assert_eq!(
        session.pcm_stage_ledger.facts().len(),
        old_ids.len() + new_ids.len()
    );
}

#[test]
fn normalized_stage_production_loop_does_not_claim_unknown_equal_length_transform() {
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    {
        let mut state = coordinator.inner.state.lock();
        state.session_id = session_id;
        state.phase = SessionPhase::Listening;
        state.cancelled = false;
    }
    let consumer: Arc<dyn crate::recorder::AudioConsumer> =
        Arc::new(DeferredBridgeTestConsumer::default());
    let mut session = embedded_audio_test_session(session_id, consumer);
    session.active_asr = "volcengine".into();
    let guard = crate::observability::begin_embedded_audio_pipeline_capture(315);
    let observation = guard.observation();
    session.attach_pipeline_observation(Some(Arc::clone(&observation)));
    let source_interval = observation
        .allocate_source_interval(session.source_stream_id, Some(101), 3_200)
        .expect("source interval");
    let source_run = super::EmbeddedStreamingPcmSourceRun {
        bytes: 3_200,
        observation: Some(Arc::clone(&observation)),
        segment_id: Some(101),
        source_interval: Some(source_interval),
        collector_metadata: None,
        collector_emitted_range: None,
    };
    session.consume_normalized_pcm_with_source_runs(
        3_200,
        &vec![6_u8; 3_200],
        &[source_run],
        Some(Arc::clone(&observation)),
        crate::observability::PcmMappingKind::Unknown,
    );
    let fact = observation
        .pcm_stage_facts_for_test()
        .into_iter()
        .find(|fact| fact.stage == crate::observability::PcmStage::Normalized)
        .expect("unknown-transform fact");
    assert_eq!(fact.mapping, crate::observability::PcmMappingKind::Unknown);
    assert_eq!(fact.source_shares[0].destination_range, None);
    assert_eq!(fact.source_shares[0].source_interval, Some(source_interval));
    assert!(observation.pcm_stage_capacity_drops_for_test().4);
}

#[test]
fn normalized_stage_production_loop_marks_coordinate_overflow_unknown() {
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    {
        let mut state = coordinator.inner.state.lock();
        state.session_id = session_id;
        state.phase = SessionPhase::Listening;
        state.cancelled = false;
    }
    let consumer: Arc<dyn crate::recorder::AudioConsumer> =
        Arc::new(DeferredBridgeTestConsumer::default());
    let mut session = embedded_audio_test_session(session_id, consumer);
    session.normalized_pcm_cursor.next = Some(u64::MAX);
    let guard = crate::observability::begin_embedded_audio_pipeline_capture(316);
    let observation = guard.observation();
    session.attach_pipeline_observation(Some(Arc::clone(&observation)));
    session.consume_normalized_pcm_with_source_runs(
        2,
        &[1_u8, 2_u8],
        &[],
        Some(Arc::clone(&observation)),
        crate::observability::PcmMappingKind::PositionPreserving,
    );
    session.consume_normalized_pcm_with_source_runs(
        2,
        &[3_u8, 4_u8],
        &[],
        Some(Arc::clone(&observation)),
        crate::observability::PcmMappingKind::PositionPreserving,
    );
    let fact = observation
        .pcm_stage_facts_for_test()
        .into_iter()
        .find(|fact| fact.stage == crate::observability::PcmStage::Normalized)
        .expect("overflow fact");
    assert_eq!(fact.destination_range, None);
    assert_eq!(fact.mapping, crate::observability::PcmMappingKind::Unknown);
    let normalized_facts = session
        .pcm_stage_ledger
        .facts()
        .into_iter()
        .filter(|fact| fact.stage == crate::observability::PcmStage::Normalized)
        .collect::<Vec<_>>();
    assert_eq!(normalized_facts.len(), 2);
    assert!(normalized_facts.iter().all(|fact| {
        fact.destination_range.is_none()
            && fact.mapping == crate::observability::PcmMappingKind::Unknown
    }));
    assert!(observation.pcm_stage_capacity_drops_for_test().4);
}

#[test]
fn explicit_mismatched_pipeline_source_does_not_reuse_previous_observation() {
    let coordinator = Coordinator::new();
    let coordinator_session_id = new_session_id();
    {
        let mut state = coordinator.inner.state.lock();
        state.session_id = coordinator_session_id;
        state.phase = SessionPhase::Listening;
        state.cancelled = false;
    }
    let target = Arc::new(DeferredBridgeTestConsumer::default());
    let consumer: Arc<dyn crate::recorder::AudioConsumer> = target.clone();
    let mut session = embedded_audio_test_session(coordinator_session_id, consumer);
    let old_guard = crate::observability::begin_embedded_audio_pipeline_capture(302);
    let old_observation = old_guard.observation();
    session.attach_pipeline_observation(Some(old_observation));

    let pcm = pcm_from_samples(&[11, -11, 22, -22]);
    session
        .consume_streaming_pcm_from_segment_with_observation(
            &coordinator.inner,
            &pcm,
            None,
            Some(42),
            None,
            true,
        )
        .expect("explicitly unassociated PCM remains audio-valid");
    session.flush_streaming_pcm();

    assert_eq!(&*target.pcm.lock().expect("test pcm lock"), &pcm);
    assert_eq!(
        &*target.sources.lock().expect("test source lock"),
        &[Some(42)]
    );
    assert_eq!(
        &*target.observations.lock().expect("test observation lock"),
        &[None]
    );
}

#[test]
fn local_confirmation_waits_for_pre_roll_plus_speech_observation() {
    assert_eq!(LOCAL_CONFIRMATION_START_MS, 800);
    assert_eq!(LOCAL_CONFIRMATION_START_BYTES / 32, 800);
}

#[test]
fn wake_phrase_tail_latency_excludes_deliberate_phrase_duration() {
    assert_eq!(super::wake_phrase_tail_to_capsule_ms(3.040, 3_216), 176);
    assert_eq!(super::wake_phrase_tail_to_capsule_ms(0.900, 1_032), 132);
    assert_eq!(super::wake_phrase_tail_to_capsule_ms(3.500, 3_200), 0);
    assert_eq!(super::wake_phrase_tail_to_capsule_ms(f32::NAN, 900), 900);
}

#[test]
fn wake_diagnostic_retention_removes_expired_then_oldest_for_bytes() {
    let now = std::time::UNIX_EPOCH + Duration::from_secs(10 * 24 * 60 * 60);
    let entries = vec![
        super::WakeDiagnosticRetentionEntry {
            path: "expired.wav".into(),
            modified: std::time::UNIX_EPOCH,
            bytes: 1,
        },
        super::WakeDiagnosticRetentionEntry {
            path: "older.wav".into(),
            modified: now - Duration::from_secs(60),
            bytes: 20 * 1024 * 1024,
        },
        super::WakeDiagnosticRetentionEntry {
            path: "newer.wav".into(),
            modified: now - Duration::from_secs(30),
            bytes: 20 * 1024 * 1024,
        },
    ];

    let removals = super::wake_diagnostic_retention_plan(
        entries,
        now,
        Duration::from_secs(7 * 24 * 60 * 60),
        128,
        32 * 1024 * 1024,
    );
    assert_eq!(
        removals,
        vec![
            std::path::PathBuf::from("expired.wav"),
            std::path::PathBuf::from("older.wav")
        ]
    );
}

#[test]
fn wake_diagnostic_cleanup_caps_matching_files_and_keeps_unrelated_files() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "listener-wake-retention-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).expect("create retention fixture");
    for index in 0..2050 {
        std::fs::write(
            directory.join(format!("wake-candidate-{index:03}.wav")),
            [index as u8],
        )
        .expect("write matching fixture");
    }
    let unrelated = directory.join("operator-note.txt");
    std::fs::write(&unrelated, b"keep").expect("write unrelated fixture");

    let removed = super::prune_default_wake_diagnostics(&directory).expect("prune fixtures");
    let remaining_wavs = std::fs::read_dir(&directory)
        .expect("read retention fixture")
        .filter_map(Result::ok)
        .filter(|item| {
            item.path()
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case("wav"))
        })
        .count();
    // 2026-09-20: caps raised 128→512 — the per-process budget exhausted
    // mid-day on live incident 2274297663, blinding forensics when needed.
    // 2026-09-21: 512 died in ~3h (index 511 at 09:41) — raised to 2048.
    assert_eq!(removed, 2);
    assert_eq!(remaining_wavs, 2048);
    assert!(unrelated.exists());

    std::fs::remove_dir_all(&directory).expect("remove retention fixture");
}

#[cfg(target_os = "windows")]
#[test]
#[ignore = "explicit offline captured-device PCM diagnostic; requires a selected installed helper"]
fn diagnostic_stage2_captured_pcm_once() {
    use sha2::{Digest, Sha256};

    fn sha256_hex(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    let input_path = std::env::var_os("LISTENER_CAPTURED_PCM_WAV")
        .map(std::path::PathBuf::from)
        .expect("LISTENER_CAPTURED_PCM_WAV must select one captured WAV");
    let report_path = std::env::var_os("LISTENER_CAPTURED_PCM_REPORT")
        .map(std::path::PathBuf::from)
        .expect("LISTENER_CAPTURED_PCM_REPORT must select the report output");
    let diagnostic_directory = std::env::var_os("LISTENER_CAPTURED_PCM_DIAGNOSTIC_DIR")
        .map(std::path::PathBuf::from)
        .expect("LISTENER_CAPTURED_PCM_DIAGNOSTIC_DIR must select the evidence directory");
    let helper_executable = std::env::var_os("LISTENER_WAKE_HELPER_EXE")
        .map(std::path::PathBuf::from)
        .expect("LISTENER_WAKE_HELPER_EXE must select the installed candidate");
    let session_id = std::env::var("LISTENER_CAPTURED_PCM_SESSION_ID")
        .expect("LISTENER_CAPTURED_PCM_SESSION_ID must identify the captured session")
        .parse::<u32>()
        .expect("LISTENER_CAPTURED_PCM_SESSION_ID must be a u32");
    let phrase = std::env::var("LISTENER_CAPTURED_PCM_WAKE_PHRASE")
        .unwrap_or_else(|_| "开始录音".to_string());

    let wav = std::fs::read(&input_path).expect("read selected captured WAV");
    let file_sha256 = sha256_hex(&wav);
    let pcm = crate::embedded_audio::read_wav_pcm16le(&wav)
        .expect("decode selected captured 16 kHz mono PCM");
    let pcm_sha256 = sha256_hex(&pcm);
    assert_eq!(pcm.len(), 192_640, "captured session PCM length changed");
    assert_eq!(pcm.len() / 32, 6_020, "captured session duration changed");
    assert_eq!(
        file_sha256,
        "3ffc06bbf4053e9d2e6ab47e3e48d1d8786094c902fe565f6880468cd72c5ebb",
        "the diagnostic must use the explicitly identified captured session"
    );

    std::fs::create_dir_all(&diagnostic_directory)
        .expect("create captured PCM diagnostic directory");
    if let Some(parent) = report_path.parent() {
        std::fs::create_dir_all(parent).expect("create captured PCM report directory");
    }
    // These are explicit diagnostic settings. Normal Listener startup keeps
    // its current_exe helper behavior and does not persist transcripts.
    std::env::set_var("LISTENER_WAKE_DIAGNOSTIC_DIR", &diagnostic_directory);
    std::env::set_var("LISTENER_WAKE_HELPER_EXE", &helper_executable);

    let cases = [
        ("origin0-5027ms", 0usize, 5_027usize),
        ("terminal-3520-6020ms", 3_520usize, 6_020usize),
        ("complete-0-6020ms", 0usize, 6_020usize),
    ];
    let case_count = cases.len();
    let mut results = Vec::with_capacity(case_count);
    for (attempt, (label, start_ms, end_ms)) in cases.iter().copied().enumerate() {
        let start = start_ms * 32;
        let end = end_ms * 32;
        assert!(end <= pcm.len() && start < end, "invalid captured PCM interval");
        let input = &pcm[start..end];
        let boosted = crate::wake_phrase::gain_normalized_pcm16(input);
        let confirmation = super::run_local_wake_confirmation_once(
            super::LocalWakeConfirmationDiagnosticContext {
                embedded_session_id: session_id,
                attempt: Some(attempt + 1),
                source_origin_bytes: start,
                branch: "captured-pcm-diagnostic",
            },
            input,
            &phrase,
            "primary",
        )
        .expect("captured PCM local confirmation");
        let (request_id, transcript_text) = super::take_last_local_wake_diagnostic_result()
            .expect("captured PCM helper diagnostic result");
        results.push(serde_json::json!({
            "label": label,
            "originMs": start_ms,
            "endMs": end_ms,
            "rawPcmBytes": input.len(),
            "boostedPcmBytes": boosted.len(),
            "rawPcmSha256": sha256_hex(input),
            "boostedPcmSha256": sha256_hex(&boosted),
            "requestId": request_id,
            "matched": confirmation.matched,
            "phraseRelation": format!("{:?}", confirmation.phrase_relation),
            "transcriptChars": confirmation.transcript_chars,
            "transcript": transcript_text,
            "phoneticPrefixUnits": confirmation.phonetic_prefix_units,
            "phoneticBestDistance": confirmation.phonetic_best_distance,
            "phoneticBestWindowStart": confirmation.phonetic_best_window_start,
            "inferenceMs": confirmation.inference_ms,
            "snapshotPcmMs": confirmation.snapshot_pcm_ms,
        }));
    }

    let report = serde_json::json!({
        "schemaVersion": 1,
        "kind": "diagnostic_stage2_captured_pcm_once",
        "inputPath": input_path,
        "inputFileBytes": wav.len(),
        "inputFileSha256": file_sha256,
        "decodedPcmBytes": pcm.len(),
        "decodedPcmMs": pcm.len() / 32,
        "decodedPcmSha256": pcm_sha256,
        "embeddedSessionId": session_id,
        "wakePhrase": phrase,
        "helperExecutable": helper_executable,
        "results": results,
    });
    std::fs::write(
        &report_path,
        serde_json::to_vec_pretty(&report).expect("encode captured PCM diagnostic report"),
    )
    .expect("write captured PCM diagnostic report");
    println!(
        "captured PCM diagnostic report={} session={} file_sha256={} cases={}",
        report_path.display(),
        session_id,
        file_sha256,
        case_count
    );
}

#[cfg(target_os = "windows")]
#[test]
#[ignore = "explicit offline source/capture/gain boundary diagnostic"]
fn diagnostic_source_capture_gain_boundary_once() {
    use sha2::{Digest, Sha256};

    fn sha256_hex(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    fn helper_result_json(result: &crate::asr::local::wake_helper::WakeHelperResult) -> serde_json::Value {
        serde_json::json!({
            "requestId": result.request_id,
            "matched": result.matched,
            "phraseRelation": format!("{:?}", result.phrase_relation),
            "transcriptChars": result.transcript_chars,
            "transcript": result.transcript_text,
            "phoneticPrefixUnits": result.phonetic_prefix_units,
            "phoneticBestDistance": result.phonetic_best_distance,
            "phoneticBestWindowStart": result.phonetic_best_window_start,
            "inferenceMs": result.inference_ms,
        })
    }

    fn write_wav(path: &std::path::Path, pcm: &[u8]) {
        std::fs::write(path, super::pcm16_wav_bytes(pcm)).expect("write diagnostic WAV");
    }

    let source_path = std::env::var_os("LISTENER_SOURCE_CAPTURE_GAIN_SOURCE_WAV")
        .map(std::path::PathBuf::from)
        .expect("LISTENER_SOURCE_CAPTURE_GAIN_SOURCE_WAV");
    let capture_path = std::env::var_os("LISTENER_SOURCE_CAPTURE_GAIN_CAPTURE_WAV")
        .map(std::path::PathBuf::from)
        .expect("LISTENER_SOURCE_CAPTURE_GAIN_CAPTURE_WAV");
    let report_path = std::env::var_os("LISTENER_SOURCE_CAPTURE_GAIN_REPORT")
        .map(std::path::PathBuf::from)
        .expect("LISTENER_SOURCE_CAPTURE_GAIN_REPORT");
    let diagnostic_directory = std::env::var_os("LISTENER_SOURCE_CAPTURE_GAIN_DIAGNOSTIC_DIR")
        .map(std::path::PathBuf::from)
        .expect("LISTENER_SOURCE_CAPTURE_GAIN_DIAGNOSTIC_DIR");
    let prior_report_path = std::env::var_os("LISTENER_SOURCE_CAPTURE_GAIN_PRIOR_REPORT")
        .map(std::path::PathBuf::from)
        .expect("LISTENER_SOURCE_CAPTURE_GAIN_PRIOR_REPORT");
    let helper_executable = std::env::var_os("LISTENER_WAKE_HELPER_EXE")
        .map(std::path::PathBuf::from)
        .expect("LISTENER_WAKE_HELPER_EXE");
    let session_id = std::env::var("LISTENER_SOURCE_CAPTURE_GAIN_SESSION_ID")
        .expect("LISTENER_SOURCE_CAPTURE_GAIN_SESSION_ID")
        .parse::<u32>()
        .expect("source/capture session id must be u32");
    let source_start_ms = std::env::var("LISTENER_SOURCE_CAPTURE_GAIN_SOURCE_START_MS")
        .expect("LISTENER_SOURCE_CAPTURE_GAIN_SOURCE_START_MS")
        .parse::<usize>()
        .expect("source start must be usize");
    let source_end_ms = std::env::var("LISTENER_SOURCE_CAPTURE_GAIN_SOURCE_END_MS")
        .expect("LISTENER_SOURCE_CAPTURE_GAIN_SOURCE_END_MS")
        .parse::<usize>()
        .expect("source end must be usize");
    let phrase = std::env::var("LISTENER_SOURCE_CAPTURE_GAIN_WAKE_PHRASE")
        .unwrap_or_else(|_| "开始录音".to_string());

    let source_wav = std::fs::read(&source_path).expect("read source WAV");
    let capture_wav = std::fs::read(&capture_path).expect("read captured WAV");
    let source = crate::embedded_audio::read_wav_pcm16le(&source_wav)
        .expect("decode source WAV");
    let capture = crate::embedded_audio::read_wav_pcm16le(&capture_wav)
        .expect("decode captured WAV");
    assert_eq!(
        sha256_hex(&source_wav),
        "8c6b1010860a4708f792338e44d784625a26f7fc6fc56d8b9665866aee5a55e6",
        "source must be the fixed S1 playback WAV"
    );
    assert_eq!(
        sha256_hex(&capture_wav),
        "3ffc06bbf4053e9d2e6ab47e3e48d1d8786094c902fe565f6880468cd72c5ebb",
        "capture must be session 265880533"
    );
    let source_start = source_start_ms * 32;
    let source_end = source_end_ms * 32;
    assert!(source_start < source_end && source_end <= source.len());
    let source_fragment = &source[source_start..source_end];
    assert!(!capture.is_empty());

    std::fs::create_dir_all(&diagnostic_directory)
        .expect("create source/capture diagnostic directory");
    if let Some(parent) = report_path.parent() {
        std::fs::create_dir_all(parent).expect("create source/capture report directory");
    }
    std::env::set_var("LISTENER_WAKE_DIAGNOSTIC_DIR", &diagnostic_directory);
    std::env::set_var("LISTENER_WAKE_HELPER_EXE", &helper_executable);

    // One raw source confirmation: this is the source/decode boundary, with
    // no Type-side gain normalization.
    let source_raw = crate::asr::local::wake_helper::confirm(
        source_fragment,
        &phrase,
        std::time::Duration::from_secs(4),
    )
    .expect("confirm source fragment without Type gain");
    write_wav(
        &diagnostic_directory.join("source-fragment-no-type-gain.wav"),
        source_fragment,
    );

    // One source confirmation through the exact production wrapper: the only
    // difference from source_raw is gain_normalized_pcm16 before the helper.
    let source_gain = super::run_local_wake_confirmation_once(
        super::LocalWakeConfirmationDiagnosticContext {
            embedded_session_id: session_id,
            attempt: Some(1),
            source_origin_bytes: source_start,
            branch: "source-capture-gain-diagnostic",
        },
        source_fragment,
        &phrase,
        "source-production-gain",
    )
    .expect("confirm source fragment through production gain");
    let (source_gain_request_id, source_gain_transcript) =
        super::take_last_local_wake_diagnostic_result()
            .expect("source production-gain helper diagnostic result");

    // One captured-candidate confirmation without Type gain. The matching
    // complete-candidate + production-gain result is read from the preceding
    // three-input report, so this card does not run that fourth inference.
    let capture_raw = crate::asr::local::wake_helper::confirm(
        &capture,
        &phrase,
        std::time::Duration::from_secs(4),
    )
    .expect("confirm captured candidate without Type gain");
    write_wav(
        &diagnostic_directory.join("capture-533-no-type-gain.wav"),
        &capture,
    );

    let prior_report = serde_json::from_slice::<serde_json::Value>(
        &std::fs::read(&prior_report_path).expect("read prior captured PCM report"),
    )
    .expect("decode prior captured PCM report");
    let prior_complete_gain = prior_report["results"]
        .as_array()
        .and_then(|rows| rows.iter().find(|row| row["label"] == "complete-0-6020ms"))
        .cloned()
        .expect("prior report complete candidate production-gain result");

    let report = serde_json::json!({
        "schemaVersion": 1,
        "kind": "diagnostic_source_capture_gain_boundary_once",
        "scope": "offline source/capture/gain boundary; no playback, device command, threshold, matcher, endpoint, or firmware change",
        "source": {
            "path": source_path,
            "fileBytes": source_wav.len(),
            "fileSha256": sha256_hex(&source_wav),
            "decodedPcmBytes": source.len(),
            "fragmentStartMs": source_start_ms,
            "fragmentEndMs": source_end_ms,
            "fragmentPcmBytes": source_fragment.len(),
            "fragmentSha256": sha256_hex(source_fragment),
            "withoutTypeGain": helper_result_json(&source_raw),
            "withProductionGain": {
                "requestId": source_gain_request_id,
                "matched": source_gain.matched,
                "phraseRelation": format!("{:?}", source_gain.phrase_relation),
                "transcriptChars": source_gain.transcript_chars,
                "transcript": source_gain_transcript,
                "phoneticPrefixUnits": source_gain.phonetic_prefix_units,
                "phoneticBestDistance": source_gain.phonetic_best_distance,
                "phoneticBestWindowStart": source_gain.phonetic_best_window_start,
                "inferenceMs": source_gain.inference_ms,
                "snapshotPcmMs": source_gain.snapshot_pcm_ms,
            },
        },
        "capture": {
            "path": capture_path,
            "fileBytes": capture_wav.len(),
            "fileSha256": sha256_hex(&capture_wav),
            "decodedPcmBytes": capture.len(),
            "decodedPcmMs": capture.len() / 32,
            "withoutTypeGain": helper_result_json(&capture_raw),
            "withProductionGainReused": prior_complete_gain,
        },
        "boundaryConclusion": {
            "sourceWithoutGainPhrase": source_raw.matched,
            "sourceWithProductionGainPhrase": source_gain.matched,
            "captureWithoutGainPhrase": capture_raw.matched,
            "captureWithProductionGainPhrase": prior_complete_gain["matched"],
            "interpretation": "The source fragment and captured candidate must be compared by the four helper outcomes. A gain-only transition can implicate Type gain; a source failure before gain implicates source/decode or source fixture; a capture-only failure with source success implicates acoustic capture/transport or source-to-capture alignment. No matcher relaxation is justified by Absent alone.",
        },
        "priorReport": prior_report_path,
        "embeddedSessionId": session_id,
        "wakePhrase": phrase,
        "acceptance": {"A": "UNRESOLVED", "P8": "FAIL", "G": "UNVERIFIED", "release": "NO-GO"},
    });
    std::fs::write(
        &report_path,
        serde_json::to_vec_pretty(&report).expect("encode source/capture/gain report"),
    )
    .expect("write source/capture/gain report");
    println!(
        "source/capture/gain report={} source_fragment_ms={}..{} capture_ms={} prior={}",
        report_path.display(),
        source_start_ms,
        source_end_ms,
        capture.len() / 32,
        prior_report_path.display()
    );
}

use std::time::{Duration, Instant};

#[derive(Default)]
struct CountingConsumer {
    bytes: AtomicUsize,
}

impl crate::recorder::AudioConsumer for CountingConsumer {
    fn consume_pcm_chunk(&self, pcm: &[u8]) {
        self.bytes.fetch_add(pcm.len(), Ordering::SeqCst);
    }
}

#[derive(Default)]
struct CapturingConsumer {
    chunks: Mutex<Vec<Vec<u8>>>,
}

impl crate::recorder::AudioConsumer for CapturingConsumer {
    fn consume_pcm_chunk(&self, pcm: &[u8]) {
        self.chunks.lock().expect("capture lock").push(pcm.to_vec());
    }
}

#[test]
fn standalone_fillers_are_removed_without_damaging_real_words() {
    assert_eq!(
        remove_standalone_dictation_fillers("嗯，呃，今天自动唤醒测试正常。"),
        "今天自动唤醒测试正常。"
    );
    assert_eq!(
        remove_standalone_dictation_fillers("今天，嗯，我要测试。"),
        "今天，我要测试。"
    );
    assert_eq!(
        remove_standalone_dictation_fillers("那个文件就是额外版本。"),
        "那个文件就是额外版本。"
    );
}

#[test]
fn filler_cleanup_preserves_structural_and_ascii_punctuation() {
    for (input, expected) in [
        ("“可以，嗯？”", "“可以？”"),
        ("可以, 嗯?", "可以?"),
        ("“可以，嗯？！”。\n下一句", "“可以？！”。\n下一句"),
        ("（嗯，可以！）", "（可以！）"),
        ("“你好！”", "“你好！”"),
        ("？！", "？！"),
        ("……嗯，继续", "继续"),
    ] {
        assert_eq!(
            remove_standalone_dictation_fillers(input),
            expected,
            "{input}"
        );
    }
}

#[test]
fn recording_transcript_preserves_wake_phrase_as_ordinary_speech() {
    assert_eq!(
        preserve_recording_transcript("开始录音，今天要说的是正文。"),
        "开始录音，今天要说的是正文。"
    );
    assert_eq!(
        preserve_recording_transcript("正常语句里开始录音只是普通内容。"),
        "正常语句里开始录音只是普通内容。"
    );
}

#[test]
fn automatic_wake_guard_removes_only_the_activation_prefix() {
    assert_eq!(
        super::strip_automatic_activation_prefix(
            "开始录音。现在开始录音又开始不灵敏。",
            "开始录音",
            false,
        ),
        "现在开始录音又开始不灵敏。"
    );
    assert_eq!(
        super::strip_automatic_activation_prefix(
            "正常语句里开始录音只是普通内容。",
            "开始录音",
            false,
        ),
        "正常语句里开始录音只是普通内容。"
    );
    assert_eq!(
        super::strip_automatic_activation_prefix(
            "嗯，开始录音，多人识别现在只保留我说的话。",
            "开始录音",
            false,
        ),
        "多人识别现在只保留我说的话。"
    );
    assert_eq!(
        super::strip_automatic_activation_prefix("嗯嗯开始录音。正文保持完整。", "开始录音", false,),
        "正文保持完整。"
    );
    assert_eq!(
        super::strip_automatic_activation_prefix(
            "额外版本，开始录音只是普通内容。",
            "开始录音",
            false,
        ),
        "额外版本，开始录音只是普通内容。"
    );
}

#[test]
fn automatic_wake_does_not_early_paste_a_long_unstripped_pre_wake_lead_in() {
    assert!(super::automatic_wake_has_unstripped_lead_in(
        "小爱同学。然后现在好像开始录音，正文继续。",
        "开始录音",
    ));
    assert!(!super::automatic_wake_has_unstripped_lead_in(
        "开始录音，正文继续。",
        "开始录音",
    ));
    assert!(!super::automatic_wake_has_unstripped_lead_in(
        "好，开始录音，正文继续。",
        "开始录音",
    ));
}

#[test]
fn automatic_wake_guard_hides_partial_prefix_and_bounded_tail() {
    assert_eq!(
        super::strip_automatic_activation_prefix("开始录", "开始录音", true),
        ""
    );
    // Live a374db12: the stream heard the accepted wake as "还是录音？"
    // before the two-pass final corrected it to "开始录音". Both spellings
    // must leave the same body so an early paste remains reconcilable.
    assert_eq!(
        super::strip_automatic_activation_prefix("还是录音？就是正文继续", "开始录音", true),
        "就是正文继续"
    );
    assert_eq!(
        super::strip_automatic_activation_prefix("开始录音就是正文继续", "开始录音", false),
        "就是正文继续"
    );
    assert_eq!(
        super::strip_automatic_activation_prefix("还是录音就是正文继续", "开始录音", true),
        "还是录音就是正文继续",
        "an unbounded near phrase may be actual body speech"
    );
    assert_eq!(
        super::strip_automatic_activation_prefix("开始收音？正文继续", "开始录音", true),
        "开始收音？正文继续",
        "a later word difference must not erase ordinary body speech"
    );
    assert_eq!(
        super::strip_automatic_activation_prefix("录音，正文开始。", "开始录音", false),
        "正文开始。"
    );
    assert_eq!(
        super::strip_automatic_activation_prefix("音频测试", "开始录音", false),
        "音频测试"
    );
    assert_eq!(
        super::strip_automatic_activation_prefix("开始。今天下午三点开会", "开始录音", false),
        "今天下午三点开会"
    );
    assert_eq!(
        super::strip_automatic_activation_prefix("开始，今天下午三点开会", "开始录音", false),
        "今天下午三点开会"
    );
    assert_eq!(
        super::strip_automatic_activation_prefix("开始我们开会", "开始录音", false),
        "开始我们开会",
        "开始 as real body must stay when 录音 is absent and no punct remnant"
    );
    assert_eq!(
        super::strip_automatic_activation_prefix("请开始录音今天下午开会", "开始录音", false),
        "今天下午开会",
        "a one-character ASR lead-in before the confirmed wake phrase must still strip"
    );
    assert_eq!(
        super::strip_automatic_activation_prefix("好，开始录音，正文留下", "开始录音", false),
        "正文留下"
    );
    assert_eq!(
        super::strip_automatic_activation_prefix(
            "正常语句里开始录音只是普通内容。",
            "开始录音",
            false,
        ),
        "正常语句里开始录音只是普通内容。",
        "mid-utterance wake words stay when they are not the activation prefix"
    );
    assert_eq!(
        super::strip_automatic_activation_prefix("下", "开始录音", true),
        "下",
        "a one-character non-prefix must not panic in lead-in scan"
    );
    let long_body = "开始录音今天下午三点开会然后我们把方案再过一遍如果没问题就按这个执行";
    assert_eq!(
        super::strip_automatic_activation_prefix(long_body, "开始录音", false),
        "今天下午三点开会然后我们把方案再过一遍如果没问题就按这个执行"
    );
    assert_eq!(
        super::strip_automatic_activation_prefix("下午", "开始录音", true),
        "下午"
    );
}

#[test]
fn remove_standalone_dictation_fillers_also_strips_inlined_chinese_fillers() {
    // 中文 ASR 常输出无标点的连续文本,语气词粘连在正文里——standalone 删不掉,
    // 这是用户觉得"开关没用"的根因。这里验证粘连的嗯/呃/唔会被剥离。
    assert_eq!(
        remove_standalone_dictation_fillers("今天嗯去测试"),
        "今天去测试"
    );
    assert_eq!(
        remove_standalone_dictation_fillers("那个嗯文件"),
        "那个文件"
    );
    assert_eq!(
        remove_standalone_dictation_fillers("呃我不知道"),
        "我不知道"
    );
    assert_eq!(remove_standalone_dictation_fillers("今天嗯嗯去"), "今天去");
    // 句首/句尾的粘连语气词也要去掉
    assert_eq!(
        remove_standalone_dictation_fillers("嗯今天嗯去嗯"),
        "今天去"
    );
    // 额有实义(额外/金额/名额),不剥离——只删被标点分隔的独立"额"
    assert_eq!(
        remove_standalone_dictation_fillers("金额是一百"),
        "金额是一百"
    );
    assert_eq!(remove_standalone_dictation_fillers("额外版本"), "额外版本");
    // 被标点分隔的独立语气词仍由 standalone 正常删除,不回归
    assert_eq!(
        remove_standalone_dictation_fillers("嗯，呃，今天测试。"),
        "今天测试。"
    );
    assert_eq!(
        remove_standalone_dictation_fillers("今天下午三点开会，呃然后我们把方案再过一遍。"),
        "今天下午三点开会，然后我们把方案再过一遍。"
    );
    assert_eq!(
        remove_standalone_dictation_fillers("开会，呃，然后继续。"),
        "开会，然后继续。"
    );
    assert_eq!(
        remove_standalone_dictation_fillers("开会，呃。然后继续。"),
        "开会。然后继续。"
    );
    assert_eq!(
        remove_standalone_dictation_fillers("这样可以，嗯？"),
        "这样可以？"
    );
    assert_eq!(
        remove_standalone_dictation_fillers("完成。\n嗯，下一项。"),
        "完成。\n下一项。"
    );
    assert_eq!(
        remove_standalone_dictation_fillers("“嗯，你好！”"),
        "“你好！”"
    );
    assert_eq!(remove_standalone_dictation_fillers("“你好！”"), "“你好！”");
    assert_eq!(
        remove_standalone_dictation_fillers("“可以，嗯？”"),
        "“可以？”"
    );
    assert_eq!(remove_standalone_dictation_fillers("可以, 嗯?"), "可以?");
    assert_eq!(
        remove_standalone_dictation_fillers("嗯，呃，今天测试。"),
        "今天测试。"
    );
    assert_eq!(
        remove_standalone_dictation_fillers("那个文件就是额外版本。"),
        "那个文件就是额外版本。"
    );
}

#[test]
fn dictation_asr_quality_warning_marks_non_core_engines() {
    assert_eq!(dictation_asr_engine_backend_id("volcengine"), "volcengine");
    assert!(dictation_asr_uses_core_accurate_engine("volcengine"));
    assert_eq!(dictation_asr_quality_warning("volcengine"), None);

    assert_eq!(
        dictation_asr_engine_backend_id("whisper"),
        "whisper-compatible"
    );
    assert!(!dictation_asr_uses_core_accurate_engine("whisper"));
    assert_eq!(
        dictation_asr_quality_warning("whisper").as_deref(),
        Some("当前识别引擎为Whisper-compatible (whisper)，不是核心 Volcengine 准确引擎，识别可能不准。")
    );
}

fn embedded_audio_test_session(
    session_id: crate::coordinator_state::SessionId,
    consumer: Arc<dyn crate::recorder::AudioConsumer>,
) -> EmbeddedAudioDictationSession {
    EmbeddedAudioDictationSession {
        session_id,
        candidate_id: None,
        source_stream_id: 9_999,
        active_asr: "openai".into(),
        consumer,
        volcengine_asr: None,
        pipeline_observation: None,
        pcm_stage_ledger: crate::observability::PcmStageMappingLedger::default(),
        pcm_stage_operation_id: Some(1),
        source_admission_operation_id: Some(1),
        candidate_fact_ledger: None,
        source_admission_ledger: Arc::new(std::sync::Mutex::new(
            super::SourceAdmissionDependencyLedger::default(),
        )),
        accepted_pcm_cursor: super::PcmDiagnosticCursor::default(),
        archive_pcm_cursor: super::PcmDiagnosticCursor::default(),
        normalized_pcm_cursor: super::PcmDiagnosticCursor::default(),
        archive_pcm: Some(Vec::new()),
        streamed_pcm_bytes: 0,
        normalized_pcm_bytes: 0,
        streaming_pcm_buffer: Vec::new(),
        streaming_pcm_sources: std::collections::VecDeque::new(),
        streaming_agc: EmbeddedStreamingAgcState::default(),
        local_speech_activity: LocalSpeechActivity::disabled(),
        local_speaker_tracker: None,
        device_ai_processing_started: false,
        proactive_stop_body_started: false,
        proactive_stop_silence_ms: 0,
        proactive_stop_dispatched: false,
    }
}

fn correction_rule(pattern: &str, replacement: &str) -> CorrectionRule {
    CorrectionRule {
        id: "test".into(),
        pattern: pattern.into(),
        replacement: replacement.into(),
        enabled: true,
        created_at: String::new(),
    }
}

fn pcm_from_samples(samples: &[i16]) -> Vec<u8> {
    samples
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect()
}

#[test]
fn embedded_pcm_visual_level_tracks_raw_voice_energy_without_asr_gain() {
    let silence = pcm_from_samples(&[0, 0, 0, 0]);
    let quiet_voice = pcm_from_samples(&[50, -50, 50, -50]);
    let ordinary_voice = pcm_from_samples(&[300, -300, 300, -300]);
    let loud_voice = pcm_from_samples(&[1_000, -1_000, 1_000, -1_000]);

    let silence_level = embedded_pcm_visual_level(&silence);
    let quiet_level = embedded_pcm_visual_level(&quiet_voice);
    let ordinary_level = embedded_pcm_visual_level(&ordinary_voice);
    let loud_level = embedded_pcm_visual_level(&loud_voice);

    assert_eq!(silence_level, 0.0);
    assert!(quiet_level > 0.012, "quiet_level={quiet_level}");
    assert!(
        ordinary_level > quiet_level,
        "{ordinary_level} <= {quiet_level}"
    );
    assert!(
        loud_level > ordinary_level,
        "{loud_level} <= {ordinary_level}"
    );
    assert_eq!(loud_level, 1.0);
}

#[test]
fn embedded_pcm_capsule_level_prefers_firmware_raw_meter_and_keeps_legacy_fallback() {
    let processed_loud = pcm_from_samples(&[2_000, -2_000, 2_000, -2_000]);
    let processed_quiet = pcm_from_samples(&[20, -20, 20, -20]);

    let quiet_raw = embedded_pcm_capsule_level(&processed_loud, Some(7));
    let loud_raw = embedded_pcm_capsule_level(&processed_quiet, Some(83));
    assert!(
        (quiet_raw - 0.03496).abs() < 0.00001,
        "quiet_raw={quiet_raw}"
    );
    assert!((loud_raw - 0.28424).abs() < 0.00001, "loud_raw={loud_raw}");
    assert_eq!(
        embedded_pcm_capsule_level(&processed_quiet, None),
        embedded_pcm_visual_level(&processed_quiet)
    );

    let sweep = [1, 21, 99].map(|level| embedded_pcm_capsule_level(&processed_loud, Some(level)));
    assert!(sweep[0] < sweep[1] && sweep[1] < sweep[2]);
    assert!(
        sweep[2] < 0.34,
        "24 dB sweep must not saturate the capsule response: {sweep:?}"
    );
}

fn samples_for_ms(ms: usize, sample: i16) -> Vec<i16> {
    vec![sample; 16_000 * ms / 1_000]
}

fn seed_cancelled_processing_session(
    coordinator: &Coordinator,
) -> crate::coordinator_state::SessionId {
    let session_id = new_session_id();
    {
        let mut state = coordinator.inner.state.lock();
        state.session_id = session_id;
        state.phase = SessionPhase::Processing;
        state.cancelled = true;
        state.focus_target = Some(42);
    }
    store_embedded_audio_stats(
        &coordinator.inner,
        crate::embedded_audio::SessionCollector::default().stats(),
    );
    session_id
}

fn assert_cancelled_processing_session_cleaned(coordinator: &Coordinator) {
    {
        let state = coordinator.inner.state.lock();
        assert_eq!(state.phase, SessionPhase::Idle);
        assert!(state.cancelled);
        assert_eq!(state.focus_target, None);
    }
    assert!(coordinator.inner.embedded_audio_stats.lock().is_none());
}

#[test]
fn finish_pipeline_error_after_processing_cancel_cleans_without_error_finish() {
    let coordinator = Coordinator::new();
    let session_id = seed_cancelled_processing_session(&coordinator);

    let finished_as_error =
        finish_dictation_pipeline_error(&coordinator.inner, session_id, "识别失败".to_string());

    assert!(!finished_as_error);
    assert_cancelled_processing_session_cleaned(&coordinator);
}

#[test]
fn failed_asr_keeps_recoverable_recording_in_history_once() {
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    {
        let mut state = coordinator.inner.state.lock();
        state.session_id = session_id;
        state.phase = SessionPhase::Processing;
        state.cancelled = false;
    }
    coordinator
        .inner
        .audio_archive_active
        .store(true, Ordering::Relaxed);
    store_embedded_audio_stats(
        &coordinator.inner,
        crate::embedded_audio::SessionCollector::default().stats(),
    );

    assert!(finish_asr_failure_with_history(
        &coordinator.inner,
        session_id,
        "识别恢复失败".to_string(),
        false,
    ));
    let history = coordinator.history().list().expect("history list");
    let matching: Vec<_> = history
        .iter()
        .filter(|session| session.id == session_id.to_string())
        .collect();
    assert_eq!(matching.len(), 1);
    assert_eq!(matching[0].error_code.as_deref(), Some("asrUnavailable"));
    assert_eq!(matching[0].has_audio_recording, Some(true));
    assert!(matching[0].final_text.is_empty());
}

#[test]
fn finish_timeout_after_processing_cancel_cleans_without_error_finish() {
    let coordinator = Coordinator::new();
    let session_id = seed_cancelled_processing_session(&coordinator);

    let finished_as_error =
        finish_dictation_timeout(&coordinator.inner, session_id, "识别超时".to_string());

    assert!(!finished_as_error);
    assert_cancelled_processing_session_cleaned(&coordinator);
}

#[test]
fn cancel_session_requests_registered_embedded_ble_capture_cancel() {
    let coordinator = Coordinator::new();
    let cancel_flag = Arc::new(AtomicBool::new(false));
    register_embedded_ble_cancel_flag(&coordinator.inner, &cancel_flag);
    {
        let mut state = coordinator.inner.state.lock();
        state.phase = SessionPhase::Listening;
        state.cancelled = false;
    }

    cancel_session(&coordinator.inner);

    assert!(cancel_flag.load(Ordering::SeqCst));
}

#[test]
fn cancel_session_does_not_stop_background_listener_cancel_handle() {
    // Owner stability bug: capsule cancel used the continuous capture cancel flag,
    // so TYPE:READY was torn down (TYPE:BYE + CCCD off) on every Esc/cancel click.
    // Continuous background registers a separate session-abort flag; the listener
    // cancel handle must stay clear so notify remains open.
    let coordinator = Coordinator::new();
    let listener_cancel = Arc::new(AtomicBool::new(false));
    let session_abort = Arc::new(AtomicBool::new(false));
    {
        *coordinator.inner.embedded_ble_listener_cancel.lock() = Some(Arc::clone(&listener_cancel));
        coordinator
            .inner
            .embedded_ble_listener_ready
            .store(true, Ordering::SeqCst);
    }
    register_embedded_ble_cancel_flag(&coordinator.inner, &session_abort);
    {
        let mut state = coordinator.inner.state.lock();
        state.phase = SessionPhase::Listening;
        state.cancelled = false;
    }

    cancel_session(&coordinator.inner);

    assert!(
        session_abort.load(Ordering::SeqCst),
        "session soft-abort must still be requested for in-flight stream cleanup"
    );
    assert!(
        !listener_cancel.load(Ordering::SeqCst),
        "continuous background notify cancel handle must not be set by dictation cancel"
    );
    assert!(
        coordinator
            .inner
            .embedded_ble_listener_ready
            .load(Ordering::SeqCst),
        "cancel must not clear TYPE:READY flag; stream soft-abort keeps notify live"
    );
}

#[test]
fn cancel_session_requests_embedded_ble_capture_cancel_even_when_idle() {
    let coordinator = Coordinator::new();
    let cancel_flag = Arc::new(AtomicBool::new(false));
    register_embedded_ble_cancel_flag(&coordinator.inner, &cancel_flag);
    coordinator.inner.state.lock().phase = SessionPhase::Idle;

    cancel_session(&coordinator.inner);

    assert!(cancel_flag.load(Ordering::SeqCst));
    let history = embedded_ble_session_actor_history(&coordinator.inner);
    assert!(history
        .iter()
        .any(|record| record.command == EmbeddedBleSessionActorCommand::CancelCommand));
}

#[test]
fn idle_cancel_without_capture_flag_does_not_route_by_default_embedded_pref() {
    let coordinator = Coordinator::new();
    coordinator.inner.state.lock().phase = SessionPhase::Idle;

    cancel_session(&coordinator.inner);

    let history = embedded_ble_session_actor_history(&coordinator.inner);
    assert!(history.is_empty());
}

#[test]
fn polish_prefetch_adoptable_only_on_exact_final_match_and_no_failure() {
    use std::collections::VecDeque;
    let make = |input: &str, result: Option<super::StreamingPolishOutcome>| PolishPrefetch {
        input: input.to_string(),
        buf: Arc::new(parking_lot::Mutex::new(PolishPrefetchBuf {
            chunks: VecDeque::new(),
            result,
        })),
        notify: Arc::new(tokio::sync::Notify::new()),
        cancel: Arc::new(AtomicBool::new(false)),
    };
    assert!(polish_prefetch_adoptable(
        &make("整理后的正文", None),
        "整理后的正文"
    ));
    assert!(
        !polish_prefetch_adoptable(&make("整理后的正文", None), "整理后的正文，多了尾巴"),
        "final with extra tail must not adopt"
    );
    assert!(!polish_prefetch_adoptable(
        &make(
            "整理后的正文",
            Some(super::StreamingPolishOutcome::Failed("idle timeout".into()))
        ),
        "整理后的正文"
    ));
}

#[tokio::test]
async fn drive_polish_prefetch_replays_buffer_then_streams_live() {
    use std::collections::VecDeque;
    let prefetch = PolishPrefetch {
        input: "正文".to_string(),
        buf: Arc::new(parking_lot::Mutex::new(PolishPrefetchBuf {
            chunks: VecDeque::from(["你".to_string(), "你好".to_string()]),
            result: None,
        })),
        notify: Arc::new(tokio::sync::Notify::new()),
        cancel: Arc::new(AtomicBool::new(false)),
    };
    let buf = Arc::clone(&prefetch.buf);
    let notify = Arc::clone(&prefetch.notify);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let drive = tokio::spawn(drive_polish_prefetch(prefetch, tx));
    // 等驱动把两个缓冲 chunk 回放完，再补一个 live chunk + 结束。
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    buf.lock().chunks.push_back("你好世".to_string());
    notify.notify_one();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    buf.lock().result = Some(super::StreamingPolishOutcome::Streamed("你好世界".into()));
    notify.notify_one();

    let outcome = tokio::time::timeout(std::time::Duration::from_secs(2), drive)
        .await
        .expect("drive must finish")
        .expect("drive task");
    let mut received = Vec::new();
    while let Ok(chunk) = rx.try_recv() {
        received.push(chunk);
    }
    assert_eq!(received, vec!["你", "你好", "你好世"]);
    match outcome {
        super::StreamingPolishOutcome::Streamed(text) => assert_eq!(text, "你好世界"),
        _ => panic!("expected Streamed outcome"),
    }
}

#[tokio::test]
async fn streaming_delivery_waits_for_typer_drain_before_sealing_submitted_text() {
    // Controlled offline timing: the provider marks its stream complete first,
    // while the fake typer is held behind a barrier. The production resolver
    // itself owns the await, so it cannot seal before the barrier is released.
    let (provider_tx, mut typer_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let provider_done = tokio::spawn(async move {
        provider_tx.send("完整流式正文".to_string()).unwrap();
    });
    let (typer_started_tx, typer_started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let typer = tokio::spawn(async move {
        let delta = typer_rx.recv().await.expect("provider delta");
        typer_started_tx.send(()).expect("typer started receiver");
        release_rx.await.expect("release typer drain");
        (delta, None)
    });

    provider_done.await.expect("provider task");
    let mut resolver = tokio::spawn(resolve_streaming_delivery_after_typer_drain(
        super::StreamingPolishOutcome::Streamed("完整流式正文".to_string()),
        "原始流式正文".to_string(),
        typer,
    ));
    typer_started_rx.await.expect("typer reached drain barrier");
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(25), &mut resolver)
            .await
            .is_err(),
        "provider completion must not seal delivery before typer drain"
    );
    release_tx.send(()).expect("release typer");
    let resolution = resolver.await.expect("resolver task");
    let prepared = match resolution {
        super::StreamingDeliveryResolution::Prepared(prepared) => prepared,
        super::StreamingDeliveryResolution::UnsupportedFallback => {
            panic!("streamed provider outcome must produce a prepared delivery")
        }
    };

    assert!(prepared.already_streamed);
    assert_eq!(prepared.intended_text, "完整流式正文");
    assert_eq!(prepared.submitted_text.as_deref(), Some("完整流式正文"));
    assert_eq!(prepared.final_text, "完整流式正文");
    assert!(prepared.polish_error.is_none());
}

#[test]
fn repeated_idle_hotkey_cancels_are_deduped_at_the_bridge() {
    // 2026-08-07 storm: 1875 idle Esc/cancels, each bouncing the background
    // listener actor (stale session cancel flag gave every one real work).
    // The bridge suppresses repeat Idle cancels; a non-Idle cancel re-arms.
    let coordinator = Coordinator::new();
    let cancel_flag = Arc::new(AtomicBool::new(false));
    register_embedded_ble_cancel_flag(&coordinator.inner, &cancel_flag);
    coordinator.inner.state.lock().phase = SessionPhase::Idle;

    let (tx, rx) = std::sync::mpsc::channel();
    let inner = std::sync::Arc::clone(&coordinator.inner);
    let handle = std::thread::spawn(move || crate::coordinator::hotkey_bridge_loop(inner, rx));
    tx.send(crate::hotkey::HotkeyEvent::Cancelled).unwrap();
    tx.send(crate::hotkey::HotkeyEvent::Cancelled).unwrap();
    drop(tx);
    handle.join().unwrap();

    let history = embedded_ble_session_actor_history(&coordinator.inner);
    let cancel_commands = history
        .iter()
        .filter(|record| record.command == EmbeddedBleSessionActorCommand::CancelCommand)
        .count();
    assert_eq!(cancel_commands, 1, "repeat Idle cancels must be deduped");
}

#[test]
fn capsule_cancel_routes_by_embedded_ble_preference_without_capture_flag() {
    let coordinator = Coordinator::new();
    {
        let mut state = coordinator.inner.state.lock();
        state.phase = SessionPhase::Listening;
        state.cancelled = false;
    }

    cancel_session(&coordinator.inner);

    let history = embedded_ble_session_actor_history(&coordinator.inner);
    assert!(history
        .iter()
        .any(|record| record.command == EmbeddedBleSessionActorCommand::CancelCommand));
}

#[test]
fn embedded_ble_cancel_registration_only_clears_matching_flag() {
    let coordinator = Coordinator::new();
    let first = Arc::new(AtomicBool::new(false));
    let second = Arc::new(AtomicBool::new(false));

    register_embedded_ble_cancel_flag(&coordinator.inner, &first);
    register_embedded_ble_cancel_flag(&coordinator.inner, &second);
    clear_embedded_ble_cancel_flag(&coordinator.inner, &first);
    assert!(Arc::ptr_eq(
        coordinator
            .inner
            .embedded_ble_cancel_flag
            .lock()
            .as_ref()
            .expect("second flag remains registered"),
        &second
    ));

    clear_embedded_ble_cancel_flag(&coordinator.inner, &second);
    assert!(coordinator.inner.embedded_ble_cancel_flag.lock().is_none());
}

#[test]
fn background_listener_keeps_notify_ready_after_completed_pipeline_failure() {
    let coordinator = Coordinator::new();
    let active = install_embedded_ble_listener_cancel(&coordinator.inner, 1);
    mark_embedded_ble_listener_ready(&coordinator.inner, &active);
    let mut streaming = EmbeddedStreamingDictation::background_listener();

    assert!(streaming.keep_notify_ready_after_completed_pipeline_error(
        &coordinator.inner,
        "ASR final failed after completed BLE audio session"
    ));

    assert!(streaming.terminal_received);
    assert!(!active.load(Ordering::SeqCst));
    assert!(embedded_ble_listener_capture_ready(&coordinator.inner));
    let history = embedded_ble_session_actor_history(&coordinator.inner);
    assert!(history.iter().any(|record| {
        record.command == EmbeddedBleSessionActorCommand::ActorRestart
            && record.detail.contains("pipeline error")
    }));
}

#[test]
fn session_actor_commands_apply_asr_cancel_timeout_and_empty_final_behavior() {
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    {
        let mut state = coordinator.inner.state.lock();
        state.session_id = session_id;
        state.phase = SessionPhase::Listening;
        state.cancelled = false;
    }
    begin_embedded_audio_preview_session(&coordinator.inner, session_id);
    let cancel_flag = Arc::new(AtomicBool::new(false));
    register_embedded_ble_cancel_flag(&coordinator.inner, &cancel_flag);

    update_embedded_audio_partial_preview(&coordinator.inner, session_id, "partial".into());
    assert_eq!(
        current_embedded_audio_partial_preview(&coordinator.inner).as_deref(),
        Some("partial")
    );
    cancel_session(&coordinator.inner);
    assert!(cancel_flag.load(Ordering::SeqCst));
    {
        let state = coordinator.inner.state.lock();
        assert_eq!(state.phase, SessionPhase::Idle);
        assert!(state.cancelled);
    }

    let timeout_session_id = new_session_id();
    {
        let mut state = coordinator.inner.state.lock();
        state.session_id = timeout_session_id;
        state.phase = SessionPhase::Processing;
        state.cancelled = false;
    }
    store_embedded_audio_stats(
        &coordinator.inner,
        crate::embedded_audio::SessionCollector::default().stats(),
    );
    assert!(publish_embedded_ble_asr_final(
        &coordinator.inner,
        timeout_session_id,
        true,
        Some("没有识别到语音".to_string())
    ));
    {
        let state = coordinator.inner.state.lock();
        assert_eq!(state.phase, SessionPhase::Idle);
    }
    {
        let mut state = coordinator.inner.state.lock();
        state.session_id = timeout_session_id;
        state.phase = SessionPhase::Processing;
        state.cancelled = false;
    }
    finish_dictation_timeout(
        &coordinator.inner,
        timeout_session_id,
        "识别超时".to_string(),
    );
    {
        let state = coordinator.inner.state.lock();
        assert_eq!(state.phase, SessionPhase::Idle);
    }

    let history = embedded_ble_session_actor_history(&coordinator.inner);
    let commands: Vec<_> = history.iter().map(|record| record.command).collect();
    assert!(commands.contains(&EmbeddedBleSessionActorCommand::AsrPartial));
    assert!(commands.contains(&EmbeddedBleSessionActorCommand::CancelCommand));
    assert!(commands.contains(&EmbeddedBleSessionActorCommand::AsrFinal));
    assert!(commands.contains(&EmbeddedBleSessionActorCommand::Timeout));
    assert!(history.windows(2).all(|pair| pair[0].seq < pair[1].seq));
}

#[tokio::test]
async fn session_actor_ble_packet_command_feeds_pcm_through_single_handler() {
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    {
        let mut state = coordinator.inner.state.lock();
        state.session_id = session_id;
        state.phase = SessionPhase::Listening;
        state.cancelled = false;
    }
    let consumer = Arc::new(CountingConsumer::default());
    let consumer_for_session: Arc<dyn crate::recorder::AudioConsumer> = consumer.clone();
    let cancel_flag = Arc::new(AtomicBool::new(false));
    register_embedded_ble_cancel_flag(&coordinator.inner, &cancel_flag);
    let mut streaming = EmbeddedStreamingDictation::default();
    streaming.embedded_session_id = Some(77);
    streaming.session = Some(embedded_audio_test_session(
        session_id,
        consumer_for_session,
    ));
    let pcm = pcm_from_samples(&samples_for_ms(100, 3_000));

    let complete = streaming
        .handle_ble_packet_actor_command(
            &coordinator.inner,
            crate::embedded_audio::StreamingSessionEvent::PcmChunk(StreamingPcmChunk {
                session_id: 77,
                packet_sequence: 0,
                pcm: pcm.clone(),
                raw_input_level_percent: Some(31),
                after_stop_boundary: false,
                metadata: None,
            }),
        )
        .await
        .expect("BLE packet actor command handles PCM");

    assert!(!complete);
    assert_eq!(consumer.bytes.load(Ordering::SeqCst), pcm.len());
    assert_eq!(
        streaming
            .session
            .as_ref()
            .expect("streaming session remains active")
            .streamed_pcm_bytes,
        pcm.len()
    );
    let history = embedded_ble_session_actor_history(&coordinator.inner);
    assert!(history
        .iter()
        .any(|record| record.command == EmbeddedBleSessionActorCommand::BlePacket));
    assert!(history.iter().any(|record| {
        record.command == EmbeddedBleSessionActorCommand::BlePacket
            && record.detail.contains("pcm_capsule")
    }));
}

#[test]
fn embedded_audio_stop_feedback_is_ble_stop_boundary() {
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    {
        let mut state = coordinator.inner.state.lock();
        state.session_id = session_id;
        state.phase = SessionPhase::Listening;
        state.cancelled = false;
    }

    assert!(emit_embedded_audio_transcribing_if_active(
        &coordinator.inner,
        session_id,
        Some("partial preview".to_string()),
    ));
    {
        let state = coordinator.inner.state.lock();
        assert_eq!(state.phase, SessionPhase::Listening);
    }

    coordinator.inner.state.lock().phase = SessionPhase::Processing;
    assert!(!emit_embedded_audio_transcribing_if_active(
        &coordinator.inner,
        session_id,
        Some("late preview".to_string()),
    ));
}

#[test]
fn key_stop_feedback_latches_transcribing_without_processing_phase() {
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    let mut prefs = crate::types::UserPreferences::default();
    prefs.dictation_input_source = DictationInputSource::Microphone;
    coordinator.inner.prefs.replace_for_tests(prefs);
    {
        let mut state = coordinator.inner.state.lock();
        state.session_id = session_id;
        state.phase = SessionPhase::Listening;
        state.cancelled = false;
    }

    assert!(request_embedded_audio_stop_feedback(
        &coordinator.inner,
        "unit_test_stop_feedback"
    ));
    assert!(embedded_audio_stop_feedback_latched(&coordinator.inner));
    {
        let state = coordinator.inner.state.lock();
        assert_eq!(state.phase, SessionPhase::Listening);
    }
}

#[tokio::test]
async fn target_speaker_endpoint_host_stop_transaction_commits_lifecycle() {
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    let cancel_flag = Arc::new(AtomicBool::new(false));
    register_embedded_ble_cancel_flag(&coordinator.inner, &cancel_flag);
    {
        let mut state = coordinator.inner.state.lock();
        state.session_id = session_id;
        state.phase = SessionPhase::Listening;
        state.cancelled = false;
    }
    assert!(coordinator
        .inner
        .recording_lifecycle
        .lock()
        .begin_manual_owner(71, session_id));
    let handled =
        request_embedded_ble_recording_stop_from_host(&coordinator.inner, "unit_test_host_stop")
            .await
            .expect("test stop request does not touch BLE transport");

    assert!(handled);
    assert!(!request_embedded_ble_recording_stop_from_host(
        &coordinator.inner,
        "unit_test_duplicate_host_stop"
    )
    .await
    .expect("duplicate stop is rejected before BLE transport"));
    assert!(!cancel_flag.load(Ordering::SeqCst));
    assert!(!embedded_audio_stop_feedback_latched(&coordinator.inner));
    {
        let state = coordinator.inner.state.lock();
        assert_eq!(state.phase, SessionPhase::Listening);
    }
    let history = embedded_ble_session_actor_history(&coordinator.inner);
    assert_eq!(
        history
            .iter()
            .filter(|record| {
                record.command == EmbeddedBleSessionActorCommand::StopCommand
                    && record.detail.contains("unit_test_host_stop")
            })
            .count(),
        1
    );
}

#[test]
fn embedded_audio_session_attaches_to_host_starting_session() {
    let coordinator = Coordinator::new();
    let mut prefs = coordinator.inner.prefs.get();
    prefs.dictation_input_source = DictationInputSource::EmbeddedBle;
    coordinator.inner.prefs.replace_for_tests(prefs);
    let cancel_flag = Arc::new(AtomicBool::new(false));
    register_embedded_ble_cancel_flag(&coordinator.inner, &cancel_flag);
    let session_id = new_session_id();
    {
        let mut state = coordinator.inner.state.lock();
        state.session_id = session_id;
        state.phase = SessionPhase::Starting;
        state.cancelled = false;
    }

    let attached = begin_embedded_audio_dictation_session_id(&coordinator.inner)
        .expect("BLE start packet should attach to the host-started session");

    assert_eq!(attached, session_id);
    let state = coordinator.inner.state.lock();
    assert_eq!(state.session_id, session_id);
    assert_eq!(state.phase, SessionPhase::Starting);
}

#[tokio::test]
async fn target_speaker_endpoint_host_stop_transaction_routes_without_capture_flag() {
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    {
        let mut state = coordinator.inner.state.lock();
        state.session_id = session_id;
        state.phase = SessionPhase::Listening;
        state.cancelled = false;
    }
    assert!(coordinator
        .inner
        .recording_lifecycle
        .lock()
        .begin_manual_owner(72, session_id));
    let handled = request_embedded_ble_recording_stop_from_host(
        &coordinator.inner,
        "unit_test_host_stop_pref",
    )
    .await
    .expect("test stop request does not touch BLE transport");

    assert!(handled);
    assert!(!embedded_audio_stop_feedback_latched(&coordinator.inner));
    {
        let state = coordinator.inner.state.lock();
        assert_eq!(state.phase, SessionPhase::Listening);
    }
    let history = embedded_ble_session_actor_history(&coordinator.inner);
    assert!(history.iter().any(|record| {
        record.command == EmbeddedBleSessionActorCommand::StopCommand
            && record.detail.contains("unit_test_host_stop_pref")
    }));
}

#[tokio::test]
async fn target_speaker_endpoint_host_stop_transport_rejects_uncommitted_session() {
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    {
        let mut state = coordinator.inner.state.lock();
        state.session_id = session_id;
        state.phase = SessionPhase::Listening;
        state.cancelled = false;
    }

    let handled = request_embedded_ble_recording_stop_from_host(
        &coordinator.inner,
        "unit_test_uncommitted_stop",
    )
    .await
    .expect("uncommitted stop is rejected before BLE transport");

    assert!(!handled);
    assert!(!embedded_ble_session_actor_history(&coordinator.inner)
        .iter()
        .any(|record| record.command == EmbeddedBleSessionActorCommand::StopCommand));
}

#[tokio::test]
async fn session_actor_stop_command_owns_embedded_ble_stop_transition() {
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    {
        let mut state = coordinator.inner.state.lock();
        state.session_id = session_id;
        state.phase = SessionPhase::Listening;
        state.cancelled = false;
    }

    end_embedded_ble_session(&coordinator.inner, true, "unit test stop command")
        .await
        .expect("stop command completes without ASR resource");

    {
        let state = coordinator.inner.state.lock();
        assert_eq!(state.phase, SessionPhase::Idle);
    }
    let history = embedded_ble_session_actor_history(&coordinator.inner);
    assert!(history.iter().any(|record| {
        record.command == EmbeddedBleSessionActorCommand::StopCommand
            && record.detail.contains("unit test stop command")
    }));
}

#[test]
fn session_actor_restart_history_covers_rapid_repeated_short_sessions() {
    let coordinator = Coordinator::new();
    let mut streaming = EmbeddedStreamingDictation::background_listener();

    assert!(streaming.keep_notify_ready_after_completed_pipeline_error(
        &coordinator.inner,
        "first short session ASR empty result"
    ));
    streaming.reset_for_next_session();
    assert!(streaming.keep_notify_ready_after_completed_pipeline_error(
        &coordinator.inner,
        "second short session polish failure"
    ));

    let restarts: Vec<_> = embedded_ble_session_actor_history(&coordinator.inner)
        .into_iter()
        .filter(|record| record.command == EmbeddedBleSessionActorCommand::ActorRestart)
        .collect();
    assert_eq!(restarts.len(), 2);
    assert!(restarts[0].seq < restarts[1].seq);
}

#[test]
fn notify_cleanup_delay_records_listener_actor_command() {
    let coordinator = Coordinator::new();
    let active = install_embedded_ble_listener_cancel(&coordinator.inner, 1);
    mark_embedded_ble_listener_ready(&coordinator.inner, &active);

    cancel_embedded_ble_listener_capture(&coordinator.inner, "notify cleanup delay test", false);

    assert!(active.load(Ordering::SeqCst));
    let history = embedded_ble_session_actor_history(&coordinator.inner);
    assert!(history.iter().any(|record| {
        record.command == EmbeddedBleSessionActorCommand::NotifyCleanupDelay
            && record.detail.contains("notify cleanup delay test")
    }));
}

#[test]
fn session_actor_diagnostics_expose_ordered_safe_event_context() {
    let coordinator = Coordinator::new();
    let session_id = new_session_id();

    record_embedded_ble_session_actor_command(
        &coordinator.inner,
        EmbeddedBleSessionActorCommand::AsrPartial,
        Some(session_id),
        "chars=12",
    );
    record_embedded_ble_session_actor_command(
        &coordinator.inner,
        EmbeddedBleSessionActorCommand::AsrFinal,
        Some(session_id),
        "transcript_empty=false",
    );

    let diagnostics = coordinator.embedded_ble_session_actor_diagnostics();
    assert_eq!(diagnostics.len(), 2);
    assert_eq!(diagnostics[0].seq, 1);
    assert_eq!(diagnostics[0].command, "asr_partial");
    assert_eq!(diagnostics[0].session_id, Some(session_id.to_string()));
    assert_eq!(diagnostics[0].detail, "chars=12");
    assert_eq!(diagnostics[1].seq, 2);
    assert_eq!(diagnostics[1].command, "asr_final");
    assert_eq!(diagnostics[1].detail, "transcript_empty=false");
}

#[test]
fn caller_cancelled_embedded_ble_stream_returns_cancel_result() {
    let result = EmbeddedStreamingDictation::default().into_cancelled_submission_result();

    assert_eq!(
        result.stats.end_reason,
        Some(crate::embedded_audio::SessionEndReason::Cancel)
    );
    assert_eq!(result.reconstructed_pcm_bytes, 0);
}

#[test]
fn embedded_ble_background_stream_has_no_idle_timeout() {
    let timeout = std::time::Duration::from_secs(120);

    assert_eq!(
        embedded_ble_stream_idle_timeout(timeout, true),
        Some(timeout)
    );
    assert_eq!(embedded_ble_stream_idle_timeout(timeout, false), None);
}

#[test]
fn embedded_streaming_pcm_after_cancel_is_not_fed_to_asr() {
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    {
        let mut state = coordinator.inner.state.lock();
        state.session_id = session_id;
        state.phase = SessionPhase::Idle;
        state.cancelled = true;
    }
    let consumer = Arc::new(CountingConsumer::default());
    let consumer_for_session: Arc<dyn crate::recorder::AudioConsumer> = consumer.clone();
    let mut session = embedded_audio_test_session(session_id, consumer_for_session);
    let pcm = pcm_from_samples(&samples_for_ms(100, 3_000));

    session
        .consume_streaming_pcm(&coordinator.inner, &pcm, None)
        .expect("cancelled PCM is ignored without error");

    assert_eq!(session.streamed_pcm_bytes, 0);
    assert_eq!(session.normalized_pcm_bytes, 0);
    assert!(!session.device_ai_processing_started);
    assert!(session.archive_pcm.as_ref().expect("archive").is_empty());
    assert_eq!(consumer.bytes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn actor_consumed_marker_uses_real_acceptance_boundary() {
    let active = Coordinator::new();
    let active_session_id = new_session_id();
    {
        let mut state = active.inner.state.lock();
        state.session_id = active_session_id;
        state.phase = SessionPhase::Listening;
        state.cancelled = false;
    }
    let active_consumer = Arc::new(CountingConsumer::default());
    let active_consumer_for_session: Arc<dyn crate::recorder::AudioConsumer> =
        active_consumer.clone();
    let mut active_session =
        embedded_audio_test_session(active_session_id, active_consumer_for_session);
    // Force the diagnostic destination coordinate to UNKNOWN. The real
    // accepted-byte counter must still classify the packet as consumed.
    active_session.accepted_pcm_cursor.next = None;
    let mut active_streaming = EmbeddedStreamingDictation::background_listener();
    active_streaming.embedded_session_id = Some(920);
    active_streaming.session = Some(active_session);
    active_streaming
        .handle_ble_packet_actor_command(
            &active.inner,
            StreamingSessionEvent::PcmChunk(StreamingPcmChunk {
                session_id: 920,
                packet_sequence: 0,
                pcm: vec![1, 2],
                raw_input_level_percent: None,
                after_stop_boundary: false,
                metadata: None,
            }),
        )
        .await
        .expect("active actor packet");
    assert!(active_streaming.last_actor_pcm_consumed);
    assert_eq!(
        active_streaming
            .session
            .as_ref()
            .expect("active session")
            .streamed_pcm_bytes,
        2
    );
    active_streaming
        .session
        .as_mut()
        .expect("active session")
        .flush_streaming_pcm();
    assert_eq!(active_consumer.bytes.load(Ordering::SeqCst), 2);

    let inactive = Coordinator::new();
    let inactive_session_id = new_session_id();
    {
        let mut state = inactive.inner.state.lock();
        state.session_id = inactive_session_id;
        state.phase = SessionPhase::Processing;
        state.cancelled = false;
    }
    let inactive_consumer = Arc::new(CountingConsumer::default());
    let inactive_consumer_for_session: Arc<dyn crate::recorder::AudioConsumer> =
        inactive_consumer.clone();
    let inactive_session =
        embedded_audio_test_session(inactive_session_id, inactive_consumer_for_session);
    super::latch_embedded_audio_stop_feedback(&inactive.inner, inactive_session_id);
    let mut inactive_streaming = EmbeddedStreamingDictation::background_listener();
    inactive_streaming.embedded_session_id = Some(921);
    inactive_streaming.session = Some(inactive_session);
    inactive_streaming
        .handle_ble_packet_actor_command(
            &inactive.inner,
            StreamingSessionEvent::PcmChunk(StreamingPcmChunk {
                session_id: 921,
                packet_sequence: 0,
                pcm: vec![3, 4],
                raw_input_level_percent: None,
                after_stop_boundary: true,
                metadata: None,
            }),
        )
        .await
        .expect("inactive actor packet is a contained no-op");
    assert!(!inactive_streaming.last_actor_pcm_consumed);
    assert_eq!(
        inactive_streaming
            .session
            .as_ref()
            .expect("inactive session")
            .streamed_pcm_bytes,
        0
    );
    assert_eq!(inactive_consumer.bytes.load(Ordering::SeqCst), 0);
}

#[test]
fn embedded_streaming_pcm_for_active_session_feeds_asr_without_early_ai_led() {
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    {
        let mut state = coordinator.inner.state.lock();
        state.session_id = session_id;
        state.phase = SessionPhase::Listening;
        state.cancelled = false;
    }
    let consumer = Arc::new(CountingConsumer::default());
    let consumer_for_session: Arc<dyn crate::recorder::AudioConsumer> = consumer.clone();
    let mut session = embedded_audio_test_session(session_id, consumer_for_session);
    let pcm = pcm_from_samples(&samples_for_ms(100, 3_000));

    session
        .consume_streaming_pcm(&coordinator.inner, &pcm, None)
        .expect("active PCM is accepted");

    assert_eq!(session.streamed_pcm_bytes, pcm.len());
    assert_eq!(session.normalized_pcm_bytes, pcm.len());
    assert!(!session.device_ai_processing_started);
    assert_eq!(session.archive_pcm.as_ref().expect("archive"), &pcm);
    assert_eq!(consumer.bytes.load(Ordering::SeqCst), pcm.len());
}

#[test]
fn embedded_streaming_pcm_combines_short_ble_packets_before_asr() {
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    {
        let mut state = coordinator.inner.state.lock();
        state.session_id = session_id;
        state.phase = SessionPhase::Listening;
        state.cancelled = false;
    }
    let consumer = Arc::new(CapturingConsumer::default());
    let consumer_for_session: Arc<dyn crate::recorder::AudioConsumer> = consumer.clone();
    let mut session = embedded_audio_test_session(session_id, consumer_for_session);
    let first_packet = pcm_from_samples(&samples_for_ms(40, 3_000));
    let second_packet = pcm_from_samples(&samples_for_ms(60, 3_000));
    let mut expected_pcm = first_packet.clone();
    expected_pcm.extend_from_slice(&second_packet);

    session
        .consume_streaming_pcm(&coordinator.inner, &first_packet, None)
        .expect("first short packet is accepted");
    assert!(consumer.chunks.lock().expect("capture lock").is_empty());
    assert_eq!(session.normalized_pcm_bytes, 0);

    session
        .consume_streaming_pcm(&coordinator.inner, &second_packet, None)
        .expect("second short packet is accepted");

    let chunks = consumer.chunks.lock().expect("capture lock");
    assert_eq!(chunks.as_slice(), [expected_pcm]);
    assert_eq!(session.streamed_pcm_bytes, EMBEDDED_AUDIO_FEED_CHUNK_BYTES);
    assert_eq!(
        session.normalized_pcm_bytes,
        EMBEDDED_AUDIO_FEED_CHUNK_BYTES
    );
    assert!(session.streaming_pcm_buffer.is_empty());
}

#[test]
fn proactive_stop_accumulates_trailing_silence_only_after_body_started() {
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    {
        let mut state = coordinator.inner.state.lock();
        state.session_id = session_id;
        state.phase = SessionPhase::Listening;
        state.cancelled = false;
    }
    let consumer = Arc::new(CountingConsumer::default());
    let consumer_for_session: Arc<dyn crate::recorder::AudioConsumer> = consumer.clone();
    let mut session = embedded_audio_test_session(session_id, consumer_for_session);

    // Leading silence before the body must not arm the proactive stop.
    let silence = pcm_from_samples(&samples_for_ms(200, 0));
    session
        .consume_streaming_pcm(&coordinator.inner, &silence, None)
        .expect("leading silence accepted");
    assert!(!session.proactive_stop_body_started);
    assert_eq!(session.proactive_stop_silence_ms, 0);

    // A voiced block starts the body and zeroes trailing silence.
    let voiced = pcm_from_samples(&samples_for_ms(100, 3_000));
    session
        .consume_streaming_pcm(&coordinator.inner, &voiced, None)
        .expect("voiced body accepted");
    assert!(session.proactive_stop_body_started);
    assert_eq!(session.proactive_stop_silence_ms, 0);

    // Trailing silence accumulates only after the body has started, but a
    // single short gap must not yet cross the proactive-stop threshold.
    session
        .consume_streaming_pcm(&coordinator.inner, &silence, None)
        .expect("trailing silence accepted");
    assert!(session.proactive_stop_silence_ms > 0);
    assert!(
        session.proactive_stop_silence_ms < EMBEDDED_STREAMING_PROACTIVE_STOP_SILENCE_MS,
        "a single 200ms gap should not yet cross the 1.2s threshold"
    );

    // Resuming speech resets the trailing-silence accumulator.
    session
        .consume_streaming_pcm(&coordinator.inner, &voiced, None)
        .expect("resume body accepted");
    assert_eq!(session.proactive_stop_silence_ms, 0);
    // The dispatcher lives in the packet handler; the session only exposes readiness.
    assert!(!session.proactive_stop_dispatched);
}

#[test]
fn silent_vad_does_not_rearm_owner_clock_from_raw_room_energy() {
    use crate::asr::volcengine::{LocalSpeechActivityState, LocalSpeechEvidence};

    let evidence = LocalSpeechEvidence {
        analyzed_through_ms: 13_600,
        analyzed_through_samples: 13_600 * 16,
        last_detected_speech_end_ms: Some(13_300),
        state: LocalSpeechActivityState::NonSpeech,
        ..Default::default()
    };
    assert!(!super::embedded_vad_supported_speech(
        true,
        evidence,
        13_600 * 16,
    ));
    assert!(!super::embedded_vad_supported_speech(
        true,
        evidence,
        (13_600 * 16) + 511,
    ));
    // A worker that has fallen behind cannot assert silence for newer audio.
    assert!(super::embedded_vad_supported_speech(
        true,
        evidence,
        (13_600 * 16) + 512,
    ));

    let resumed = LocalSpeechEvidence {
        analyzed_through_ms: 14_400,
        analyzed_through_samples: 14_400 * 16,
        last_detected_speech_end_ms: Some(14_400),
        state: LocalSpeechActivityState::Speech,
        ..evidence
    };
    assert!(super::embedded_vad_supported_speech(
        false,
        resumed,
        14_400 * 16,
    ));
    assert!(super::embedded_vad_supported_speech(
        true,
        LocalSpeechEvidence {
            state: LocalSpeechActivityState::PendingSpeech,
            ..resumed
        },
        14_400 * 16,
    ));
}

#[test]
fn failed_asr_uses_only_the_bounded_local_silence_fallback() {
    assert_eq!(
        super::proactive_stop_silence_threshold_ms(true),
        1_200,
        "a failed provider must not leave an accepted recording open forever"
    );
    assert_eq!(
        super::proactive_stop_silence_threshold_ms(false),
        EMBEDDED_STREAMING_PROACTIVE_STOP_SILENCE_MS,
        "healthy ASR must not have a second raw-energy endpoint path"
    );
}

#[test]
fn every_body_preview_uses_the_same_one_second_owner_inactivity_contract() {
    let base = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("1".into()),
        target_speech_end_ms: Some(1_500),
        provider_audio_duration_ms: Some(2_500),
        audio_duration_ms: Some(2_500),
        local_speech_end_ms: Some(1_500),
        qualified_owner_speech_end_ms: None,
        qualified_owner_activity_advanced: false,
        local_speaker_classification_kind: None,
        local_speaker_signal_quality_sufficient: None,
        local_speaker_observation_end_ms: None,
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: None,
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: false,
        stable_attributed_speech_end_ms: Some(1_500),
        target_activity_advanced: false,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    // The product contract is the 3.0 s window (2026-09-23): a 1 s gap is not
    // yet due even under a 2.0 s window; the 3 s gap is due under both.
    assert!(!super::target_speaker_endpoint_due_with_timeout(
        &base, 2_000
    ));

    let wider_window_due = crate::asr::volcengine::TargetSpeakerUpdate {
        provider_audio_duration_ms: Some(4_500),
        audio_duration_ms: Some(4_500),
        ..base
    };
    assert!(super::target_speaker_endpoint_due(&wider_window_due));
    assert!(super::target_speaker_endpoint_due_with_timeout(
        &wider_window_due,
        2_000
    ));
    assert_eq!(
        super::target_speaker_inactive_stop_reason(1_000),
        "target_speaker_inactive_3000ms"
    );
    // Continued owner speech rearms the clock. Text shape never changes the
    // public one-second inactivity contract.
    assert_eq!(
        super::target_speaker_end_timeout_ms_for_preview(Some("用全刷。")),
        super::EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS
    );
    assert_eq!(
        super::target_speaker_end_timeout_ms_for_preview(Some("简单说一下。")),
        super::EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS
    );
    assert_eq!(
        super::target_speaker_end_timeout_ms_for_preview(Some("现在整体是一个什么进度？")),
        super::EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS
    );
    assert_eq!(
        super::target_speaker_end_timeout_ms_for_preview(Some("我先检查一下，然后。")),
        super::EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS,
    );
    assert_eq!(
        super::target_speaker_end_timeout_ms_for_preview(Some("最后。")),
        super::EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS,
    );
    assert_eq!(
        super::target_speaker_end_timeout_ms_for_preview(Some("最后一句要完整。")),
        super::EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS,
        "ordinary complete sentences use the one-second quiet contract",
    );
    assert!(super::preview_has_dangling_continuation(Some(
        "这部分已经完成，但是。"
    )));
    assert!(!super::preview_has_dangling_continuation(Some(
        "这部分已经完成。"
    )));
    assert!(super::preview_has_dangling_continuation(Some(
        "We can continue, and."
    )));
    assert!(!super::preview_has_dangling_continuation(Some(
        "This is a brand."
    )));
    assert_eq!(
        super::target_speaker_end_timeout_ms_for_preview(Some("你继续帮我看一下吧")),
        super::EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS
    );
    assert_eq!(
        super::target_speaker_end_timeout_ms_for_preview(Some("现在是进入")),
        super::EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS
    );
    assert_eq!(
        super::target_speaker_end_timeout_ms_for_preview(Some("那你")),
        super::EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS
    );
    assert!(super::preview_ends_with_sentence_terminal(Some(
        "现在整体是一个什么进度？你跟我简单说一下。"
    )));
    assert!(!super::preview_ends_with_sentence_terminal(Some(
        "你继续帮我看一下吧。就是他进入"
    )));
}

#[test]
fn manual_open_clause_uses_bounded_continuation_stage() {
    let started = std::time::Instant::now();
    let update = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: None,
        target_speech_end_ms: None,
        provider_audio_duration_ms: None,
        audio_duration_ms: Some(0),
        local_speech_end_ms: Some(0),
        qualified_owner_speech_end_ms: None,
        qualified_owner_activity_advanced: false,
        local_speaker_classification_kind: None,
        local_speaker_signal_quality_sufficient: None,
        local_speaker_observation_end_ms: None,
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: None,
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: false,
        stable_attributed_speech_end_ms: None,
        target_activity_advanced: false,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: false,
    };
    let mut clock = super::SettledTargetEndpointClock::default();
    clock.manual_vad_guard = true;
    clock.note_local_vad_evidence(crate::asr::volcengine::LocalSpeechEvidence {
        analyzed_through_ms: 0,
        last_detected_speech_end_ms: Some(0),
        activity_epoch: 1,
        state: crate::asr::volcengine::LocalSpeechActivityState::NonSpeech,
        ..Default::default()
    });
    clock.note_visible_body_boundary(false, 16, started);
    let generation = clock
        .observe(&update, true, started)
        .expect("manual visible body arms endpoint");

    assert!(clock
        .latest_due_update(
            started + std::time::Duration::from_millis(1_000),
            1_000,
        )
        .is_none(), "an open clause must enter continuation pending at the first due point");
    assert!(clock.continuation_pending_active(
        started + std::time::Duration::from_millis(1_500)
    ));
    clock.note_visible_body_boundary(false, 32, started + std::time::Duration::from_millis(1_100));
    assert!(clock
        .arm_latest_for_visible_body(
            started + std::time::Duration::from_millis(1_100),
            true,
        )
        .is_none(), "provider preview growth must not reset the same continuation deadline");
    assert!(clock
        .due_update(
            generation,
            started + std::time::Duration::from_millis(2_999),
            1_000,
        )
        .is_none(), "the continuation stage is bounded but not yet expired");
    assert!(clock
        .due_update(
            generation,
            started + std::time::Duration::from_millis(3_000),
            1_000,
        )
        .is_some(), "the fixed total continuation cutoff must still stop");
}

#[test]
fn automatic_wake_rhetorical_question_pause_keeps_continuation() {
    // 2026-09-19 12:36Z live shape: the user paused ~1.2 s after a rhetorical
    // question ("那继续吧，然后哦，你看一下怎么弄哦？" = 18 visible chars,
    // punctuated TERMINAL) and resumed with "我快点把…".  The old gate treated
    // the ？ as a session end, refused the continuation window, and the 1 s
    // clock cut the resumed tail.  A conversational ？ invites continuation;
    // only manual hotkey sessions keep the non-terminal-only contract.
    let started = std::time::Instant::now();
    let update = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: None,
        target_speech_end_ms: None,
        provider_audio_duration_ms: None,
        audio_duration_ms: Some(0),
        local_speech_end_ms: Some(0),
        qualified_owner_speech_end_ms: None,
        qualified_owner_activity_advanced: false,
        local_speaker_classification_kind: None,
        local_speaker_signal_quality_sufficient: None,
        local_speaker_observation_end_ms: None,
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: None,
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: false,
        stable_attributed_speech_end_ms: None,
        target_activity_advanced: false,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: false,
    };
    let mut clock = super::SettledTargetEndpointClock::default();
    clock.automatic_wake_session = true;
    clock.note_visible_body_boundary(true, 18, started);
    let generation = clock
        .observe(&update, true, started)
        .expect("terminal visible body arms endpoint");

    assert!(
        clock
            .latest_due_update(
                started + std::time::Duration::from_millis(1_000),
                1_000,
            )
            .is_none(),
        "a rhetorical question pause must enter continuation pending, not cut at 1 s"
    );
    assert!(clock.continuation_pending_active(
        started + std::time::Duration::from_millis(1_200)
    ));
    assert!(
        clock
            .due_update(
                generation,
                started + std::time::Duration::from_millis(3_000),
                1_000,
            )
            .is_some(),
        "the continuation stage stays bounded"
    );
}

#[test]
fn automatic_wake_dangling_pause_survives_missing_or_stale_local_edge() {
    // 2026-09-19 13:42Z live shape (session f564b094): the body grew to 24
    // visible chars ending in the dangling filler "…我说一句话，然后那个",
    // the provider sealed its utterance final, and at the 1 s due check the
    // governing update carried NO usable local speech edge (None in one
    // shape, an edge 4.7 s stale in the other).  The continuation anchor then
    // had nothing — or an already-expired deadline — so the cutoff latched
    // and `target_speaker_inactive_1000ms` fired while the user was saying
    // the next word ("绿…"), ending the session early and swallowing it.
    // A missing or long-stale local edge falls back to the live audio edge,
    // while the total window remains three seconds from the owner arm.
    for (label, due_local_speech_end_ms) in [("missing", None), ("stale", Some(1_200))] {
        let started = std::time::Instant::now();
        let growing_update = crate::asr::volcengine::TargetSpeakerUpdate {
            speaker_id: None,
            target_speech_end_ms: None,
            provider_audio_duration_ms: None,
            audio_duration_ms: Some(5_300),
            local_speech_end_ms: Some(5_200),
            qualified_owner_speech_end_ms: None,
            qualified_owner_activity_advanced: false,
            local_speaker_classification_kind: None,
            local_speaker_signal_quality_sufficient: None,
            local_speaker_observation_end_ms: None,
            local_tentative_owner_speech_end_ms: None,
            local_target_speech_end_ms: None,
            local_non_target_speech_end_ms: None,
            local_speaker_tracking_enabled: false,
            stable_attributed_speech_end_ms: None,
            target_activity_advanced: false,
            pending_unattributed_speech: false,
            pending_activity_advanced: false,
            speaker_info_present: false,
        };
        let due_update = crate::asr::volcengine::TargetSpeakerUpdate {
            audio_duration_ms: Some(5_900),
            local_speech_end_ms: due_local_speech_end_ms,
            ..growing_update.clone()
        };
        let mut clock = super::SettledTargetEndpointClock::default();
        clock.automatic_wake_session = true;
        clock.note_visible_body_boundary(false, 24, started);
        let generation = clock
            .observe(&growing_update, true, started)
            .expect("automatic visible body arms endpoint");
        // The provider's utterance final for the dangling clause arrives with
        // the unusable local edge, then one more visible growth refreshes the
        // positive-evidence budget — exactly the live ordering at 13:42Z.
        clock.observe(&due_update, true, started + std::time::Duration::from_millis(700));
        clock.note_visible_body_boundary(false, 25, started + std::time::Duration::from_millis(800));

        assert!(
            clock
                .latest_due_update(
                    started + std::time::Duration::from_millis(1_000),
                    1_000,
                )
                .is_none(),
            "{label}: the dangling pause must hold via the audio-edge fallback instead of cutting at 1 s"
        );
        assert!(
            clock.continuation_pending_active(started + std::time::Duration::from_millis(1_200)),
            "{label}: continuation pending entered"
        );
        assert!(
            clock
                .due_update(
                    generation,
                    started + std::time::Duration::from_millis(2_999),
                    1_000,
                )
                .is_none(),
            "{label}: the fallback window is bounded but not yet expired"
        );
        assert!(
            clock
                .due_update(
                    generation,
                    started + std::time::Duration::from_millis(3_000),
                    1_000,
                )
                .is_some(),
            "{label}: the fallback continuation must still stop"
        );
    }
}

#[test]
fn automatic_wake_open_clause_gets_bounded_continuation() {
    let started = std::time::Instant::now();
    let update = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: None,
        target_speech_end_ms: None,
        provider_audio_duration_ms: None,
        audio_duration_ms: Some(0),
        local_speech_end_ms: Some(0),
        qualified_owner_speech_end_ms: None,
        qualified_owner_activity_advanced: false,
        local_speaker_classification_kind: None,
        local_speaker_signal_quality_sufficient: None,
        local_speaker_observation_end_ms: None,
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: None,
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: false,
        stable_attributed_speech_end_ms: None,
        target_activity_advanced: false,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: false,
    };
    let mut clock = super::SettledTargetEndpointClock::default();
    // Defect A shape: an accepted automatic wake, no manual VAD sidecar, and a
    // flowing non-terminal body well past the short-command threshold.  The
    // user is mid-sentence in a quiet room (silence confirmed by nothing —
    // automatic sessions never receive local VAD evidence).
    clock.automatic_wake_session = true;
    clock.note_visible_body_boundary(false, 24, started);
    let generation = clock
        .observe(&update, true, started)
        .expect("automatic visible body arms endpoint");

    assert!(clock
        .latest_due_update(
            started + std::time::Duration::from_millis(1_000),
            1_000,
        )
        .is_none(), "an established open body must enter continuation pending instead of cutting at 1 s");
    assert!(clock.continuation_pending_active(
        started + std::time::Duration::from_millis(1_500)
    ));
    assert!(clock
        .due_update(
            generation,
            started + std::time::Duration::from_millis(2_999),
            1_000,
        )
        .is_none(), "the continuation stage is bounded but not yet expired");
    assert!(clock
        .due_update(
            generation,
            started + std::time::Duration::from_millis(3_000),
            1_000,
        )
        .is_some(), "the fixed total continuation cutoff must still stop");
}

#[test]
fn automatic_wake_short_open_body_keeps_fast_one_second_contract() {
    let started = std::time::Instant::now();
    let update = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: None,
        target_speech_end_ms: None,
        provider_audio_duration_ms: None,
        audio_duration_ms: Some(0),
        local_speech_end_ms: Some(0),
        qualified_owner_speech_end_ms: None,
        qualified_owner_activity_advanced: false,
        local_speaker_classification_kind: None,
        local_speaker_signal_quality_sufficient: None,
        local_speaker_observation_end_ms: None,
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: None,
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: false,
        stable_attributed_speech_end_ms: None,
        target_activity_advanced: false,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: false,
    };
    let mut clock = super::SettledTargetEndpointClock::default();
    clock.automatic_wake_session = true;
    // A short command whose tail the provider has not punctuated yet must not
    // inherit the continuation window: finished short utterances keep the
    // fast one-second auto-end.
    clock.note_visible_body_boundary(false, 8, started);
    let generation = clock
        .observe(&update, true, started)
        .expect("short visible body arms endpoint");
    assert!(clock
        .due_update(
            generation,
            started + std::time::Duration::from_millis(2_999),
            1_000,
        )
        .is_none());
    assert!(clock
        .due_update(
            generation,
            started + std::time::Duration::from_millis(3_000),
            1_000,
        )
        .is_some(), "a short open body below the threshold still stops at the ordinary endpoint");
    assert!(!clock.continuation_pending_active(
        started + std::time::Duration::from_millis(1_200)
    ));
}

#[test]
fn automatic_wake_continuation_rearms_after_owner_resumes() {
    let started = std::time::Instant::now();
    let update = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: None,
        target_speech_end_ms: None,
        provider_audio_duration_ms: None,
        audio_duration_ms: Some(0),
        local_speech_end_ms: Some(0),
        qualified_owner_speech_end_ms: None,
        qualified_owner_activity_advanced: false,
        local_speaker_classification_kind: None,
        local_speaker_signal_quality_sufficient: None,
        local_speaker_observation_end_ms: None,
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: None,
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: false,
        stable_attributed_speech_end_ms: None,
        target_activity_advanced: false,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: false,
    };
    let mut clock = super::SettledTargetEndpointClock::default();
    clock.automatic_wake_session = true;
    clock.note_visible_body_boundary(false, 24, started);
    let _ = clock.observe(&update, true, started).expect("endpoint armed");
    assert!(clock
        .latest_due_update(
            started + std::time::Duration::from_millis(1_000),
            1_000,
        )
        .is_none());
    assert!(clock.continuation_pending_active(
        started + std::time::Duration::from_millis(1_100)
    ));

    // The user resumes inside the window.  Automatic sessions have no local
    // VAD sidecar, so the strictly newer local speech edge is the resume
    // signal: it must cancel the pending continuation and clear the cutoff
    // latch, or every later mid-sentence pause in the same session would cut
    // at 1 s.
    let resumed = crate::asr::volcengine::TargetSpeakerUpdate {
        audio_duration_ms: Some(1_400),
        local_speech_end_ms: Some(1_400),
        ..update
    };
    let resumed_at = started + std::time::Duration::from_millis(1_500);
    assert!(clock.observe(&resumed, true, resumed_at).is_some());
    assert!(
        !clock.continuation_pending_active(resumed_at),
        "resume cancels the pending continuation"
    );
    assert!(
        !clock.continuation_cutoff_reached,
        "resume clears the cutoff latch so the next pause earns a fresh window"
    );
}

#[test]
fn tracked_bystander_speech_cannot_restart_the_three_second_continuation() {
    let started = std::time::Instant::now();
    let owner = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: None,
        target_speech_end_ms: None,
        provider_audio_duration_ms: None,
        audio_duration_ms: Some(0),
        local_speech_end_ms: Some(0),
        qualified_owner_speech_end_ms: Some(0),
        qualified_owner_activity_advanced: false,
        local_speaker_classification_kind: Some(
            crate::asr::volcengine::LocalSpeakerClassificationKind::Target,
        ),
        local_speaker_signal_quality_sufficient: Some(true),
        local_speaker_observation_end_ms: Some(0),
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: Some(0),
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: None,
        target_activity_advanced: false,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: false,
    };
    let mut clock = super::SettledTargetEndpointClock::default();
    clock.automatic_wake_session = true;
    clock.note_visible_body_boundary(false, 24, started);
    clock.observe(&owner, true, started).expect("owner arms endpoint");
    assert!(clock
        .latest_due_update(started + std::time::Duration::from_millis(1_000), 1_000)
        .is_none());

    let bystander = crate::asr::volcengine::TargetSpeakerUpdate {
        audio_duration_ms: Some(1_400),
        local_speech_end_ms: Some(1_400),
        local_non_target_speech_end_ms: Some(1_400),
        local_speaker_classification_kind: Some(
            crate::asr::volcengine::LocalSpeakerClassificationKind::NonTarget,
        ),
        local_speaker_observation_end_ms: Some(1_400),
        local_tentative_owner_speech_end_ms: None,
        ..owner
    };
    clock.observe(
        &bystander,
        true,
        started + std::time::Duration::from_millis(1_500),
    );
    assert!(clock.continuation_pending_active(
        started + std::time::Duration::from_millis(1_500)
    ));
    assert!(clock
        .latest_due_update(started + std::time::Duration::from_millis(3_000), 1_000)
        .is_some(), "room speech must not buy a second three-second window");
}

#[test]
fn cloud_activity_loop_cannot_outlive_positive_evidence_budget() {
    // 2026-09-19 17:4x live shape: a settled session whose room kept tripping
    // the local energy detector while a cloud row absorbed those edges into
    // the target id.  rearm reason=provider_activity_advanced reset the
    // one-second clock every ~1.7 s and local_speech_end_ms stayed pinned to
    // the live edge with classification=None, so auto-end never fired and 3
    // of 5 sessions needed a manual stop ("不能自动结束").  Cloud-only
    // activity must not re-arm, hold, or veto the stop once the
    // positive-evidence budget (qualified/Target advance or preview growth)
    // has expired.
    let started = std::time::Instant::now();
    let speaking = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("7".to_string()),
        target_speech_end_ms: Some(1_000),
        provider_audio_duration_ms: Some(1_000),
        audio_duration_ms: Some(1_000),
        local_speech_end_ms: Some(1_000),
        qualified_owner_speech_end_ms: None,
        qualified_owner_activity_advanced: false,
        local_speaker_classification_kind: None,
        local_speaker_signal_quality_sufficient: None,
        local_speaker_observation_end_ms: None,
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: None,
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: None,
        target_activity_advanced: true,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    let mut clock = super::SettledTargetEndpointClock::default();
    clock.automatic_wake_session = true;
    clock.note_visible_body_boundary(false, 30, started);
    clock
        .observe(&speaking, true, started)
        .expect("established body arms endpoint");

    // The loop: every 1.5 s a cloud row advances the target boundary, local
    // edges ride the live audio edge, classification stays None, the preview
    // stays settled (no growth), and no qualified/Target watermark moves.
    for step in 1..=4u64 {
        let at = started + std::time::Duration::from_millis(1_500 * step);
        let looping = crate::asr::volcengine::TargetSpeakerUpdate {
            target_speech_end_ms: Some(1_000 + 1_500 * step),
            provider_audio_duration_ms: Some(1_000 + 1_500 * step),
            audio_duration_ms: Some(1_000 + 1_500 * step),
            local_speech_end_ms: Some(1_000 + 1_500 * step),
            ..speaking.clone()
        };
        let _ = clock.observe(&looping, true, at);
    }
    assert!(clock
        .latest_due_update(
            started + std::time::Duration::from_millis(2_000),
            super::EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS,
        )
        .is_none(), "inside the budget the ordinary rearm cadence still governs");
    assert!(clock
        .latest_due_update(
            started + std::time::Duration::from_millis(6_000),
            super::EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS,
        )
        .is_some(), "an expired positive-evidence budget must let the endpoint stop through the cloud-activity loop");
}

#[test]
fn manual_continuation_stage_rearms_on_new_local_speech_edge() {
    let started = std::time::Instant::now();
    let update = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: None,
        target_speech_end_ms: None,
        provider_audio_duration_ms: None,
        audio_duration_ms: Some(0),
        local_speech_end_ms: Some(0),
        qualified_owner_speech_end_ms: None,
        qualified_owner_activity_advanced: false,
        local_speaker_classification_kind: None,
        local_speaker_signal_quality_sufficient: None,
        local_speaker_observation_end_ms: None,
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: None,
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: false,
        stable_attributed_speech_end_ms: None,
        target_activity_advanced: false,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: false,
    };
    let mut clock = super::SettledTargetEndpointClock::default();
    clock.manual_vad_guard = true;
    clock.note_local_vad_evidence(crate::asr::volcengine::LocalSpeechEvidence {
        analyzed_through_ms: 0,
        last_detected_speech_end_ms: Some(0),
        activity_epoch: 1,
        state: crate::asr::volcengine::LocalSpeechActivityState::NonSpeech,
        ..Default::default()
    });
    clock.note_visible_body_boundary(false, 16, started);
    let _ = clock.observe(&update, true, started).expect("endpoint armed");
    assert!(clock
        .latest_due_update(
            started + std::time::Duration::from_millis(1_000),
            1_000,
        )
        .is_none());

    let resumed = crate::asr::volcengine::TargetSpeakerUpdate {
        audio_duration_ms: Some(1_400),
        local_speech_end_ms: Some(1_400),
        ..update
    };
    let resumed_at = started + std::time::Duration::from_millis(1_500);
    clock.note_local_vad_evidence(crate::asr::volcengine::LocalSpeechEvidence {
        analyzed_through_ms: 1_400,
        last_detected_speech_end_ms: Some(0),
        activity_epoch: 2,
        state: crate::asr::volcengine::LocalSpeechActivityState::PendingSpeech,
        ..Default::default()
    });
    assert!(clock.observe(&resumed, true, resumed_at).is_none());
    assert!(clock.continuation_pending_active(resumed_at));

    // PendingSpeech and canonical Speech share the same activity epoch. The
    // state transition itself must still be observable as a confirmed new
    // interval and reset the endpoint window.
    clock.note_local_vad_evidence(crate::asr::volcengine::LocalSpeechEvidence {
        analyzed_through_ms: 1_400,
        last_detected_speech_end_ms: Some(0),
        activity_epoch: 2,
        state: crate::asr::volcengine::LocalSpeechActivityState::Speech,
        ..Default::default()
    });
    assert!(clock
        .observe(
            &resumed,
            true,
            started + std::time::Duration::from_millis(1_600),
        )
        .is_some());
    assert!(!clock.continuation_pending_active(
        started + std::time::Duration::from_millis(1_600)
    ));
    assert!(clock
        .latest_due_update(
            started + std::time::Duration::from_millis(2_499),
            1_000,
        )
        .is_none(), "new speech must reset the endpoint window instead of inheriting the old cutoff");
}

#[test]
fn canonical_speech_after_continuation_cutoff_can_start_the_next_window() {
    let started = std::time::Instant::now();
    let initial = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: None,
        target_speech_end_ms: None,
        provider_audio_duration_ms: None,
        audio_duration_ms: Some(0),
        local_speech_end_ms: Some(0),
        qualified_owner_speech_end_ms: None,
        qualified_owner_activity_advanced: false,
        local_speaker_classification_kind: None,
        local_speaker_signal_quality_sufficient: None,
        local_speaker_observation_end_ms: None,
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: None,
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: false,
        stable_attributed_speech_end_ms: None,
        target_activity_advanced: false,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: false,
    };
    let mut clock = super::SettledTargetEndpointClock::default();
    clock.manual_vad_guard = true;
    clock.note_local_vad_evidence(crate::asr::volcengine::LocalSpeechEvidence {
        analyzed_through_ms: 0,
        last_detected_speech_end_ms: Some(0),
        activity_epoch: 1,
        state: crate::asr::volcengine::LocalSpeechActivityState::NonSpeech,
        ..Default::default()
    });
    clock.note_visible_body_boundary(false, 16, started);
    clock.observe(&initial, true, started).expect("endpoint armed");
    assert!(clock
        .latest_due_update(
            started + std::time::Duration::from_millis(1_000),
            1_000,
        )
        .is_none());

    let pending = crate::asr::volcengine::TargetSpeakerUpdate {
        audio_duration_ms: Some(2_200),
        local_speech_end_ms: Some(2_200),
        ..initial
    };
    let pending_at = started + std::time::Duration::from_millis(2_200);
    clock.note_local_vad_evidence(crate::asr::volcengine::LocalSpeechEvidence {
        analyzed_through_ms: 2_200,
        last_detected_speech_end_ms: Some(0),
        activity_epoch: 2,
        state: crate::asr::volcengine::LocalSpeechActivityState::PendingSpeech,
        ..Default::default()
    });
    assert!(clock.observe(&pending, true, pending_at).is_none());
    assert!(clock
        .due_update(
            1,
            started + std::time::Duration::from_millis(2_500),
            1_000,
        )
        .is_none(), "cutoff must not discard a still-active PendingSpeech interval");

    // PendingSpeech and canonical Speech share one activity epoch. The serial
    // transition, retained after cutoff, must still reopen the next window.
    clock.note_local_vad_evidence(crate::asr::volcengine::LocalSpeechEvidence {
        analyzed_through_ms: 2_400,
        last_detected_speech_end_ms: Some(0),
        activity_epoch: 2,
        state: crate::asr::volcengine::LocalSpeechActivityState::Speech,
        ..Default::default()
    });
    assert!(clock
        .observe(
            &crate::asr::volcengine::TargetSpeakerUpdate {
                audio_duration_ms: Some(2_400),
                local_speech_end_ms: Some(2_400),
                ..pending
            },
            true,
            started + std::time::Duration::from_millis(2_600),
        )
        .is_some());
    assert!(!clock.continuation_cutoff_reached);
}

#[test]
fn endpoint_stop_requires_audio_time_silence_not_only_wall_clock_silence() {
    let mut evidence = crate::asr::volcengine::LocalSpeechEvidence {
        analyzed_through_ms: 8_029,
        last_detected_speech_end_ms: Some(5_030),
        state: crate::asr::volcengine::LocalSpeechActivityState::NonSpeech,
        ..Default::default()
    };
    assert!(!super::local_vad_stop_silence_is_qualified(&evidence));
    evidence.analyzed_through_ms = 8_030;
    assert!(super::local_vad_stop_silence_is_qualified(&evidence));
    evidence.last_detected_speech_end_ms = None;
    assert!(!super::local_vad_stop_silence_is_qualified(&evidence));
}

#[test]
fn stale_provider_snapshot_is_expired_by_single_session_reducer() {
    let started = std::time::Instant::now();
    let update = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("0".into()),
        target_speech_end_ms: Some(6_502),
        provider_audio_duration_ms: Some(7_100),
        audio_duration_ms: Some(7_200),
        local_speech_end_ms: Some(6_900),
        qualified_owner_speech_end_ms: Some(2_400),
        qualified_owner_activity_advanced: true,
        local_speaker_classification_kind: Some(crate::asr::volcengine::LocalSpeakerClassificationKind::Target),
        local_speaker_signal_quality_sufficient: Some(true),
        local_speaker_observation_end_ms: Some(2_400),
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: Some(2_400),
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(6_502),
        target_activity_advanced: true,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    let mut clock = super::SettledTargetEndpointClock::default();
    let generation = clock.observe(&update, true, started).expect("armed");

    // No provider/local callback arrives after the stable row. The reducer
    // must expire that frozen tail at the one-second deadline instead of
    // leaving the session in arbiter_hold forever.
    let stopped = clock.due_update(
        generation,
        started + std::time::Duration::from_millis(900),
        900,
    );
    assert!(stopped.is_some(), "stale endpoint evidence must stop");
}

#[test]
fn visible_preview_seeds_endpoint_before_first_diarization_row() {
    let started = std::time::Instant::now();
    let snapshot = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: None,
        target_speech_end_ms: None,
        provider_audio_duration_ms: Some(1_000),
        audio_duration_ms: Some(1_000),
        local_speech_end_ms: Some(1_000),
        qualified_owner_speech_end_ms: None,
        qualified_owner_activity_advanced: false,
        local_speaker_classification_kind: None,
        local_speaker_signal_quality_sufficient: None,
        local_speaker_observation_end_ms: None,
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: None,
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: None,
        target_activity_advanced: false,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: false,
    };
    let mut clock = super::SettledTargetEndpointClock::default();
    clock.seed_from_snapshot_if_missing(snapshot, true, started);
    assert_eq!(
        clock.lifecycle(),
        crate::speech_decision_kernel::OwnerEndpointState::QuietPending,
        "visible provider text must create a bounded endpoint even before diarization"
    );
    assert!(clock
        .latest_due_update(started + std::time::Duration::from_millis(900), 900)
        .is_some());
}

#[test]
fn settled_target_wall_clock_ends_one_second_after_visible_stable_text() {
    let started = std::time::Instant::now();
    let stable = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("1".into()),
        target_speech_end_ms: Some(4_082),
        provider_audio_duration_ms: Some(4_600),
        audio_duration_ms: Some(4_700),
        local_speech_end_ms: Some(4_700),
        qualified_owner_speech_end_ms: None,
        qualified_owner_activity_advanced: false,
        local_speaker_classification_kind: None,
        local_speaker_signal_quality_sufficient: None,
        local_speaker_observation_end_ms: None,
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: None,
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(4_082),
        target_activity_advanced: true,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    let mut clock = super::SettledTargetEndpointClock::default();
    let generation = clock
        .observe(&stable, true, started)
        .expect("stable visible owner text arms the wall clock");

    let noisy_local_update = crate::asr::volcengine::TargetSpeakerUpdate {
        audio_duration_ms: Some(7_900),
        local_speech_end_ms: Some(7_900),
        target_activity_advanced: false,
        ..stable
    };
    assert_eq!(
        clock.observe(
            &noisy_local_update,
            true,
            started + std::time::Duration::from_millis(999),
        ),
        None,
        "low-level local energy must not rearm settled owner text",
    );
    assert!(clock
        .due_update(
            generation,
            started + std::time::Duration::from_millis(999),
            1_000,
        )
        .is_none());
    assert!(clock
        .due_update(
            generation,
            started + std::time::Duration::from_millis(1_000),
            1_000,
        )
        .is_some());
    assert!(
        clock
            .latest_due_update(started + std::time::Duration::from_millis(1_000), 1_000,)
            .is_none(),
        "the product endpoint is an exactly-once terminal decision"
    );
}

#[test]
fn settled_target_provider_boundary_regression_does_not_restart_deadline() {
    // Provider diarization can briefly publish 8292 -> 6452 -> 8292 while
    // preview callbacks continue. The regressed row is not fresh owner text;
    // accepting it as a re-arm would move the endpoint forever.
    let started = std::time::Instant::now();
    let stable = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("0".into()),
        target_speech_end_ms: Some(8_292),
        provider_audio_duration_ms: Some(8_900),
        audio_duration_ms: Some(9_000),
        local_speech_end_ms: Some(8_900),
        qualified_owner_speech_end_ms: None,
        qualified_owner_activity_advanced: false,
        local_speaker_classification_kind: None,
        local_speaker_signal_quality_sufficient: None,
        local_speaker_observation_end_ms: None,
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: None,
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: false,
        stable_attributed_speech_end_ms: Some(8_292),
        target_activity_advanced: true,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    let mut clock = super::SettledTargetEndpointClock::default();
    let generation = clock
        .observe(&stable, true, started)
        .expect("stable provider boundary arms endpoint");
    let regressed = crate::asr::volcengine::TargetSpeakerUpdate {
        target_speech_end_ms: Some(6_452),
        stable_attributed_speech_end_ms: Some(6_452),
        target_activity_advanced: false,
        ..stable.clone()
    };
    assert_eq!(
        clock.observe(
            &regressed,
            true,
            started + std::time::Duration::from_millis(700),
        ),
        None,
        "a regressed provider boundary must not reset the wall clock"
    );
    assert!(clock
        .due_update(
            generation,
            started + std::time::Duration::from_millis(2_899),
            900,
        )
        .is_none());
    assert!(clock
        .due_update(
            generation,
            started + std::time::Duration::from_millis(2_900),
            900,
        )
        .is_some());
}

#[test]
fn settled_target_wall_clock_does_not_cut_a_fresh_unattributed_owner_tail() {
    // Real hardware regression f45064eb: provider text settled near 9.7 s,
    // then its utterance-boundary frame carried no new text while local PCM
    // and speech continued through 11.38 s. The old wall-clock path ignored
    // that fresh local speech and stopped halfway through the spoken sentence.
    let started = std::time::Instant::now();
    let stable = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("0".into()),
        target_speech_end_ms: Some(9_700),
        provider_audio_duration_ms: Some(9_700),
        audio_duration_ms: Some(9_700),
        local_speech_end_ms: Some(9_700),
        qualified_owner_speech_end_ms: None,
        qualified_owner_activity_advanced: false,
        local_speaker_classification_kind: None,
        local_speaker_signal_quality_sufficient: None,
        local_speaker_observation_end_ms: None,
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: None,
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: false,
        stable_attributed_speech_end_ms: Some(9_700),
        target_activity_advanced: true,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    let mut clock = super::SettledTargetEndpointClock::default();
    clock.note_visible_body_boundary(false, 33, started);
    let generation = clock
        .observe(&stable, true, started)
        .expect("stable visible owner text arms the wall clock");

    let continuing_local_speech = crate::asr::volcengine::TargetSpeakerUpdate {
        provider_audio_duration_ms: Some(10_400),
        audio_duration_ms: Some(12_380),
        local_speech_end_ms: Some(12_380),
        target_activity_advanced: false,
        ..stable
    };
    assert_eq!(
        clock.observe(
            &continuing_local_speech,
            true,
            started + std::time::Duration::from_millis(850),
        ),
        None,
        "local speech does not re-arm the settled-text deadline",
    );
    assert!(clock
        .due_update(
            generation,
            started + std::time::Duration::from_millis(1_000),
            1_000,
        )
        .is_none());
    assert!(clock
        .latest_due_update(started + std::time::Duration::from_millis(1_000), 1_000)
        .is_none());

    let confirmed_other = crate::asr::volcengine::TargetSpeakerUpdate {
        local_non_target_speech_end_ms: Some(12_380),
        local_speaker_tracking_enabled: true,
        ..continuing_local_speech
    };
    assert_eq!(
        clock.observe(
            &confirmed_other,
            true,
            started + std::time::Duration::from_millis(1_010),
        ),
        None,
    );
    assert!(
        clock
            .latest_due_update(started + std::time::Duration::from_millis(1_010), 1_000)
            .is_some(),
        "confirmed other speech must not hold the owner endpoint",
    );
}

#[test]
fn settled_target_wall_clock_rearms_on_fresh_local_owner_boundary() {
    // Installed multi-interference session c96a7159: cloud diarization stayed
    // at 3182 ms while the enrolled local verifier confirmed the owner through
    // 4500 ms. At local audio 4600 ms the old wall clock was already due and
    // stopped in the middle of the second sentence.
    let started = std::time::Instant::now();
    let first_owner = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("0".into()),
        target_speech_end_ms: Some(3_182),
        provider_audio_duration_ms: Some(3_500),
        audio_duration_ms: Some(3_600),
        local_speech_end_ms: Some(3_600),
        qualified_owner_speech_end_ms: Some(2_900),
        qualified_owner_activity_advanced: true,
        local_speaker_classification_kind: Some(crate::asr::volcengine::LocalSpeakerClassificationKind::Target),
        local_speaker_signal_quality_sufficient: Some(true),
        local_speaker_observation_end_ms: Some(2_900),
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: Some(2_900),
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(3_182),
        target_activity_advanced: true,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    let mut clock = super::SettledTargetEndpointClock::default();
    clock.note_visible_body_boundary(true, 8, started);
    let obsolete_generation = clock
        .observe(&first_owner, true, started)
        .expect("first owner boundary arms endpoint");

    let continuing_owner = crate::asr::volcengine::TargetSpeakerUpdate {
        provider_audio_duration_ms: Some(4_300),
        audio_duration_ms: Some(4_600),
        local_speech_end_ms: Some(4_600),
        local_target_speech_end_ms: Some(4_500),
        target_activity_advanced: false,
        ..first_owner
    };
    let current_generation = clock
        .observe(
            &continuing_owner,
            true,
            started + std::time::Duration::from_millis(965),
        )
        .expect("fresh local Target boundary must rearm endpoint");
    assert_ne!(current_generation, obsolete_generation);
    assert!(clock
        .due_update(
            obsolete_generation,
            started + std::time::Duration::from_millis(1_000),
            900,
        )
        .is_none());
    assert!(clock
        .due_update(
            current_generation,
            started + std::time::Duration::from_millis(3_964),
            900,
        )
        .is_none());
    assert!(clock
        .due_update(
            current_generation,
            started + std::time::Duration::from_millis(3_965),
            900,
        )
        .is_some());
}

#[test]
fn delivered_body_starts_one_three_second_window_and_late_preview_does_not_restart_it() {
    use std::time::{Duration, Instant};

    let started = Instant::now();
    let owner = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("0".into()),
        target_speech_end_ms: Some(3_000),
        provider_audio_duration_ms: Some(3_500),
        audio_duration_ms: Some(3_600),
        local_speech_end_ms: Some(3_000),
        qualified_owner_speech_end_ms: Some(3_000),
        qualified_owner_activity_advanced: true,
        local_speaker_classification_kind: Some(crate::asr::volcengine::LocalSpeakerClassificationKind::Target),
        local_speaker_signal_quality_sufficient: Some(true),
        local_speaker_observation_end_ms: Some(3_000),
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: Some(3_000),
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(3_000),
        target_activity_advanced: true,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    let mut clock = super::SettledTargetEndpointClock::default();
    clock.automatic_wake_session = true;
    clock.note_visible_body_boundary(false, 40, started);
    let generation = clock.observe(&owner, true, started).expect("owner arms clock");
    let delivered_at = started + Duration::from_millis(500);
    clock.note_body_delivery(delivered_at);
    let late_preview_at = started + Duration::from_millis(1_000);
    // b72c67ea: a two-pass row published after paste inflated local_target
    // to the current capture edge while the qualified owner edge stayed old.
    let delayed_row = crate::asr::volcengine::TargetSpeakerUpdate {
        audio_duration_ms: Some(4_100),
        provider_audio_duration_ms: Some(4_100),
        local_target_speech_end_ms: Some(4_100),
        qualified_owner_activity_advanced: false,
        target_activity_advanced: true,
        ..owner
    };
    assert_eq!(clock.observe(&delayed_row, true, late_preview_at), None);
    clock.note_visible_body_boundary(true, 42, late_preview_at);
    assert_eq!(clock.arm_latest_for_visible_body(late_preview_at, true), None);
    assert_eq!(clock.armed_at, Some(started));
    assert!(clock
        .due_update(generation, delivered_at + Duration::from_millis(2_999), 2_900)
        .is_none());
    assert!(clock
        .due_update(generation, delivered_at + Duration::from_millis(3_000), 2_900)
        .is_some());
}

#[test]
fn delivered_body_window_renews_for_owner_speech_but_not_a_bystander() {
    use std::time::{Duration, Instant};

    let started = Instant::now();
    let owner = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("0".into()),
        target_speech_end_ms: Some(3_000),
        provider_audio_duration_ms: Some(3_500),
        audio_duration_ms: Some(3_600),
        local_speech_end_ms: Some(3_000),
        qualified_owner_speech_end_ms: Some(3_000),
        qualified_owner_activity_advanced: true,
        local_speaker_classification_kind: Some(crate::asr::volcengine::LocalSpeakerClassificationKind::Target),
        local_speaker_signal_quality_sufficient: Some(true),
        local_speaker_observation_end_ms: Some(3_000),
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: Some(3_000),
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(3_000),
        target_activity_advanced: true,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    let make_clock = || {
        let mut clock = super::SettledTargetEndpointClock::default();
        clock.automatic_wake_session = true;
        clock.note_visible_body_boundary(true, 40, started);
        clock.observe(&owner, true, started).expect("owner arms clock");
        clock.note_body_delivery(started + Duration::from_millis(500));
        clock
    };

    let mut bystander_clock = make_clock();
    let bystander = crate::asr::volcengine::TargetSpeakerUpdate {
        audio_duration_ms: Some(4_300),
        local_speech_end_ms: Some(4_200),
        local_non_target_speech_end_ms: Some(4_200),
        local_speaker_classification_kind: Some(crate::asr::volcengine::LocalSpeakerClassificationKind::NonTarget),
        local_speaker_observation_end_ms: Some(4_200),
        local_tentative_owner_speech_end_ms: None,
        qualified_owner_activity_advanced: false,
        target_activity_advanced: false,
        ..owner.clone()
    };
    assert_eq!(bystander_clock.observe(&bystander, true, started + Duration::from_millis(1_000)), None);
    assert_eq!(bystander_clock.armed_at, Some(started));

    let mut continuing_clock = make_clock();
    let continuing_owner = crate::asr::volcengine::TargetSpeakerUpdate {
        audio_duration_ms: Some(4_300),
        local_speech_end_ms: Some(4_200),
        qualified_owner_speech_end_ms: Some(4_200),
        local_target_speech_end_ms: Some(4_200),
        local_speaker_observation_end_ms: Some(4_200),
        local_tentative_owner_speech_end_ms: None,
        qualified_owner_activity_advanced: true,
        target_activity_advanced: false,
        ..owner
    };
    assert!(continuing_clock
        .observe(&continuing_owner, true, started + Duration::from_millis(1_000))
        .is_some());
    assert_eq!(continuing_clock.armed_at, Some(started + Duration::from_millis(1_000)));
}

#[test]
fn delivered_body_waits_briefly_for_a_local_owner_candidate_without_renewing_on_room_speech() {
    use std::time::{Duration, Instant};

    let started = Instant::now();
    let owner = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("0".into()),
        target_speech_end_ms: Some(3_000),
        provider_audio_duration_ms: Some(3_500),
        audio_duration_ms: Some(3_600),
        local_speech_end_ms: Some(3_000),
        qualified_owner_speech_end_ms: Some(3_000),
        qualified_owner_activity_advanced: true,
        local_speaker_classification_kind: Some(crate::asr::volcengine::LocalSpeakerClassificationKind::Target),
        local_speaker_signal_quality_sufficient: Some(true),
        local_speaker_observation_end_ms: Some(3_000),
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: Some(3_000),
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(3_000),
        target_activity_advanced: true,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    let delivered_at = started + Duration::from_millis(500);
    let make_clock = || {
        let mut clock = super::SettledTargetEndpointClock::default();
        clock.automatic_wake_session = true;
        clock.note_visible_body_boundary(true, 40, started);
        let generation = clock.observe(&owner, true, started).expect("owner arms endpoint");
        clock.note_body_delivery(delivered_at);
        (clock, generation)
    };
    // Session 561fe6a9: the 11.5s local window was Target before the
    // three-second boundary, but its signal quality became sufficient only
    // after STOP. Captured audio had already advanced by one raw frame, so
    // the ordinary current-window label was hidden. This explicit candidate
    // carries the classified boundary without claiming confirmed ownership.
    let candidate_at = delivered_at + Duration::from_millis(2_300);
    let candidate = crate::asr::volcengine::TargetSpeakerUpdate {
        audio_duration_ms: Some(6_300),
        local_speech_end_ms: Some(6_300),
        local_speaker_classification_kind: None,
        local_speaker_signal_quality_sufficient: None,
        local_speaker_observation_end_ms: Some(6_200),
        local_tentative_owner_speech_end_ms: Some(6_200),
        qualified_owner_activity_advanced: false,
        target_activity_advanced: false,
        ..owner.clone()
    };
    let (mut clock, generation) = make_clock();
    assert_eq!(clock.observe(&candidate, true, candidate_at), None);
    assert_eq!(clock.armed_at, Some(started), "candidate cannot restart the three-second clock");
    assert!(clock.due_update(generation, delivered_at + Duration::from_millis(3_200), 2_900).is_none());
    let confirmed = crate::asr::volcengine::TargetSpeakerUpdate {
        audio_duration_ms: Some(6_700),
        local_speech_end_ms: Some(6_700),
        qualified_owner_speech_end_ms: Some(6_600),
        qualified_owner_activity_advanced: true,
        local_speaker_observation_end_ms: Some(6_600),
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: Some(6_600),
        ..candidate.clone()
    };
    assert!(clock.observe(&confirmed, true, delivered_at + Duration::from_millis(3_300)).is_some());
    assert!(!clock.stop_proposed);

    let (mut timeout_clock, timeout_generation) = make_clock();
    timeout_clock.observe(&candidate, true, candidate_at);
    assert!(timeout_clock.due_update(timeout_generation, candidate_at + Duration::from_millis(super::TENTATIVE_OWNER_CONTINUATION_MAX_WAIT_MS - 1), 2_900).is_none());
    assert!(timeout_clock.due_update(timeout_generation, candidate_at + Duration::from_millis(super::TENTATIVE_OWNER_CONTINUATION_MAX_WAIT_MS), 2_900).is_some(), "unconfirmed candidate cannot hold recording forever");

    let (mut bystander_clock, bystander_generation) = make_clock();
    bystander_clock.observe(&candidate, true, candidate_at);
    let bystander = crate::asr::volcengine::TargetSpeakerUpdate {
        audio_duration_ms: Some(6_500),
        local_speech_end_ms: Some(6_500),
        local_non_target_speech_end_ms: Some(6_500),
        local_tentative_owner_speech_end_ms: None,
        ..candidate
    };
    bystander_clock.observe(&bystander, true, delivered_at + Duration::from_millis(2_700));
    assert!(bystander_clock.due_update(bystander_generation, delivered_at + Duration::from_millis(3_000), 2_900).is_some(), "confirmed room speech must not extend the owner's window");
}

#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
#[test]
fn heavy_wake_separation_requires_independent_partial_phrase_evidence() {
    assert!(!super::target_wake_extraction_has_weak_phrase_evidence(
        false, false, 0,
    ));
    assert!(super::target_wake_extraction_has_weak_phrase_evidence(
        false, true, 0,
    ));
    assert!(super::target_wake_extraction_has_weak_phrase_evidence(
        false, false, 1,
    ));
    assert!(super::target_wake_extraction_has_weak_phrase_evidence(
        true, false, 0,
    ));
}

#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
#[test]
fn phrase_owner_mismatch_is_routed_to_separated_recovery_before_reject() {
    let owner_gate = include_str!("dictation_wake_owner_gate.rs");
    let stream = include_str!("dictation_embedded_stream.rs").replace("\r\n", "\n");
    assert!(owner_gate.contains("maybe_start_phrase_owner_recovery"));
    assert!(owner_gate.contains("\"phrase_owner_mismatch\""));
    // Terminal recovery must also run when phrase evidence exists but the
    // mixed full-buffer owner check failed; otherwise overlap is rejected
    // before the separated waveform can be evaluated.
    assert!(stream.contains(
        "|| (!enrolled_owner_matched\n                    && phrase_signal != denzic_voice_activation_v1_core::PhraseSignal::None)"
    ));
    assert!(owner_gate.contains("phrase_evidence: bool"));
}

#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
#[test]
fn terminal_lazy_wake_recovery_gets_its_own_bounded_budget() {
    assert_eq!(
        super::target_wake_extraction_terminal_wait_ms(false),
        super::TARGET_WAKE_EXTRACTION_PREFETCHED_WAIT_MS
    );
    assert_eq!(
        super::target_wake_extraction_terminal_wait_ms(true),
        super::TARGET_WAKE_EXTRACTION_LAZY_TERMINAL_WAIT_MS
    );
    assert!(
        super::TARGET_WAKE_EXTRACTION_LAZY_TERMINAL_WAIT_MS
            > super::TARGET_WAKE_EXTRACTION_PREFETCHED_WAIT_MS
    );
}

#[test]
fn installed_session_531_terminal_preview_does_not_cut_continuing_enrolled_owner() {
    let started = std::time::Instant::now();
    let owner = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("0".into()),
        target_speech_end_ms: Some(2_762),
        provider_audio_duration_ms: Some(3_100),
        audio_duration_ms: Some(3_200),
        local_speech_end_ms: Some(3_200),
        qualified_owner_speech_end_ms: Some(2_700),
        qualified_owner_activity_advanced: true,
        local_speaker_classification_kind: Some(crate::asr::volcengine::LocalSpeakerClassificationKind::Target),
        local_speaker_signal_quality_sufficient: Some(true),
        local_speaker_observation_end_ms: Some(2_700),
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: Some(2_700),
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(2_762),
        target_activity_advanced: true,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    let mut clock = super::SettledTargetEndpointClock::default();
    clock.note_visible_body_boundary(true, 14, started);
    let generation = clock
        .observe(&owner, true, started)
        .expect("terminal owner preview arms the ordinary endpoint clock");

    let continuing_owner = crate::asr::volcengine::TargetSpeakerUpdate {
        provider_audio_duration_ms: Some(4_000),
        audio_duration_ms: Some(4_000),
        local_speech_end_ms: Some(4_000),
        target_activity_advanced: false,
        ..owner.clone()
    };
    assert_eq!(
        clock.observe(
            &continuing_owner,
            true,
            started + std::time::Duration::from_millis(850),
        ),
        None,
    );
    assert!(
        clock
            .due_update(
                generation,
                started + std::time::Duration::from_millis(900),
                900,
            )
            .is_none(),
        "terminal punctuation must not override live enrolled-owner speech",
    );

    let owner_now_quiet = crate::asr::volcengine::TargetSpeakerUpdate {
        provider_audio_duration_ms: Some(5_000),
        audio_duration_ms: Some(5_000),
        local_speech_end_ms: Some(4_000),
        ..continuing_owner.clone()
    };
    assert_eq!(
        clock.observe(
            &owner_now_quiet,
            true,
            started + std::time::Duration::from_millis(1_000),
        ),
        None,
    );
    assert!(
        clock
            .latest_due_update(started + std::time::Duration::from_millis(3_000), 900)
            .is_some(),
        "the owner's trailing silence must still end the session at the endpoint window",
    );

    let confirmed_other = crate::asr::volcengine::TargetSpeakerUpdate {
        local_non_target_speech_end_ms: Some(4_000),
        ..continuing_owner
    };
    let mut other_clock = super::SettledTargetEndpointClock::default();
    other_clock.note_visible_body_boundary(true, 14, started);
    let other_generation = other_clock
        .observe(&owner, true, started)
        .expect("owner preview arms the other-speaker control clock");
    other_clock.observe(
        &confirmed_other,
        true,
        started + std::time::Duration::from_millis(850),
    );
    assert!(
        other_clock
            .due_update(
                other_generation,
                started + std::time::Duration::from_millis(900),
                900,
            )
            .is_some(),
        "confirmed other speech must not hold the owner's recording open",
    );
}

#[test]
fn settled_target_wall_clock_bridges_a_manual_terminal_supplement_without_slowing_short_commands() {
    let started = std::time::Instant::now();
    let stable = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("0".into()),
        target_speech_end_ms: Some(16_332),
        provider_audio_duration_ms: Some(16_800),
        audio_duration_ms: Some(16_800),
        local_speech_end_ms: Some(16_800),
        qualified_owner_speech_end_ms: None,
        qualified_owner_activity_advanced: false,
        local_speaker_classification_kind: None,
        local_speaker_signal_quality_sufficient: None,
        local_speaker_observation_end_ms: None,
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: None,
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: false,
        stable_attributed_speech_end_ms: Some(16_332),
        target_activity_advanced: true,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    let mut clock = super::SettledTargetEndpointClock::default();
    clock.note_visible_body_boundary(false, 40, started);
    clock
        .observe(&stable, true, started)
        .expect("open manual preview arms the wall clock");

    let terminal_at = started + std::time::Duration::from_millis(100);
    // Real hardware session 7db3f128 first exposed a 31-character open
    // provider preview, then speaker attribution shrank it to a 6-character
    // terminal supplement while speech and PCM were still advancing.
    clock.note_visible_body_boundary(true, 6, terminal_at);
    let generation = clock
        .arm_latest_for_visible_body(terminal_at, true)
        .expect("terminal supplement rearms the visible-body clock");
    let continuing = crate::asr::volcengine::TargetSpeakerUpdate {
        provider_audio_duration_ms: Some(18_400),
        audio_duration_ms: Some(18_500),
        local_speech_end_ms: Some(18_500),
        target_activity_advanced: false,
        ..stable.clone()
    };
    assert_eq!(
        clock.observe(
            &continuing,
            true,
            started + std::time::Duration::from_millis(900),
        ),
        None,
    );
    assert!(clock
        .due_update(
            generation,
            terminal_at + std::time::Duration::from_millis(3_799),
            900,
        )
        .is_none());
    assert!(clock
        .due_update(
            generation,
            terminal_at + std::time::Duration::from_millis(3_800),
            900,
        )
        .is_some());

    let mut terminal_first = super::SettledTargetEndpointClock::default();
    terminal_first.note_visible_body_boundary(false, 3, started);
    terminal_first.note_visible_body_boundary(true, 4, started);
    let terminal_first_short_command = crate::asr::volcengine::TargetSpeakerUpdate {
        target_speech_end_ms: Some(942),
        provider_audio_duration_ms: Some(4_400),
        audio_duration_ms: Some(4_500),
        local_speech_end_ms: Some(4_400),
        stable_attributed_speech_end_ms: Some(3_902),
        ..stable
    };
    let short_generation = terminal_first
        .observe(&terminal_first_short_command, true, started)
        .expect("terminal-first short command arms normally");
    // 2026-09-23 3 s endpoint contract: a short command's session close is the
    // 3 s silence window itself; perceived completion speed is carried by
    // pause-early text landing at the ~1 s stability point, not by this stop.
    assert!(terminal_first
        .due_update(
            short_generation,
            started + std::time::Duration::from_millis(2_899),
            900,
        )
        .is_none());
    assert!(terminal_first
        .due_update(
            short_generation,
            started + std::time::Duration::from_millis(2_900),
            900,
        )
        .is_some());
}

#[test]
fn settled_target_wall_clock_keeps_scheduling_allowance_below_public_endpoint() {
    assert_eq!(super::EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS, 3_000);
    assert_eq!(super::EMBEDDED_SETTLED_TARGET_WALL_CLOCK_MS, 2_900);
    assert!(
        super::EMBEDDED_SETTLED_TARGET_WALL_CLOCK_MS
            < super::EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS
    );
}

#[test]
fn target_speaker_endpoint_does_not_commit_before_current_voiceprint_result() {
    // Replay the ordering from installed session
    // c5f6bdc7-e8f2-4649-9349-b5809b201608: provider/owner evidence was
    // settled, but a classification for already captured body audio was still
    // running when the 900 ms wall clock expired.
    let started = std::time::Instant::now();
    let update = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("0".into()),
        target_speech_end_ms: Some(3_122),
        provider_audio_duration_ms: Some(6_200),
        audio_duration_ms: Some(6_200),
        local_speech_end_ms: Some(3_100),
        qualified_owner_speech_end_ms: Some(2_400),
        qualified_owner_activity_advanced: true,
        local_speaker_classification_kind: Some(crate::asr::volcengine::LocalSpeakerClassificationKind::Target),
        local_speaker_signal_quality_sufficient: Some(true),
        local_speaker_observation_end_ms: Some(2_400),
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: Some(2_400),
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(3_122),
        target_activity_advanced: true,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    let mut clock = super::SettledTargetEndpointClock::default();
    clock.note_visible_body_boundary(true, 11, started);
    clock
        .observe(&update, true, started)
        .expect("settled owner arms endpoint");

    assert!(clock
        .latest_due_update_after_owner_analysis(
            started + std::time::Duration::from_millis(900),
            900,
            true,
        )
        .is_none());
    assert_eq!(
        clock.take_due_hold_diagnostic(),
        Some((clock.generation, "owner_analysis_in_flight")),
    );

    assert!(clock
        .latest_due_update_after_owner_analysis(
            started + std::time::Duration::from_millis(1_100),
            900,
            false,
        )
        .is_some());
}

#[test]
fn target_speaker_endpoint_wake_interference_baseline_requests_only_separated_verification() {
    let mut first_sample = super::WakeInterferenceBaseline::default();
    assert!(
        first_sample.observe(0.34, false),
        "a terminal candidate may have only one owner snapshot"
    );

    let mut baseline = super::WakeInterferenceBaseline::default();
    for score in [0.10, 0.11, 0.18, 0.14] {
        assert!(!baseline.observe(score, false));
    }
    assert!(baseline.observe(0.34, false));
    // A mixed-path phrase hit already has a lightweight route and neither
    // trains nor invokes the no-phrase recovery trigger.
    assert!(!baseline.observe(0.60, true));
    // A likely-owner outlier is deliberately not learned into room baseline.
    assert!(baseline.observe(0.33, false));
}

#[test]
fn target_speaker_endpoint_wake_interference_baseline_is_candidate_scoped() {
    let source = include_str!("dictation_wake_polish.rs");
    assert!(
        source.contains("wake_interference_baseline: WakeInterferenceBaseline"),
        "interference calibration must live on each buffered candidate"
    );
    assert!(
        !source.contains("static WAKE_INTERFERENCE_BASELINE")
            && !source.contains("with_wake_interference_baseline"),
        "process-global interference calibration is a cross-session wake bypass"
    );
}

#[test]
fn continuation_text_does_not_override_the_owner_activity_clock() {
    assert!(
        super::EMBEDDED_DANGLING_FIRMWARE_KEEPALIVE_INTERVAL_MS
            + (super::EMBEDDED_ASR_SPEECH_ACTIVITY_TIMEOUT.as_millis() as u64)
            < super::EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS,
        "firmware keepalive must finish before its one-second silence fallback"
    );
    assert_eq!(
        super::target_speaker_end_timeout_ms_for_preview(Some("我先看一下，然后")),
        super::EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS
    );
    assert_eq!(
        super::target_speaker_end_timeout_ms_for_preview(Some("我已经说完了。")),
        super::EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS
    );
    assert_eq!(
        super::target_speaker_end_timeout_ms_for_preview(Some("普通一句话")),
        super::EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS
    );
}

#[test]
fn enrolled_noise_tail_cannot_hold_settled_owner_past_uncertainty_ceiling() {
    let started = std::time::Instant::now();
    let owner = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("0".into()),
        target_speech_end_ms: Some(5_602),
        provider_audio_duration_ms: Some(6_500),
        audio_duration_ms: Some(6_500),
        local_speech_end_ms: Some(6_500),
        qualified_owner_speech_end_ms: None,
        qualified_owner_activity_advanced: false,
        local_speaker_classification_kind: None,
        local_speaker_signal_quality_sufficient: None,
        local_speaker_observation_end_ms: None,
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: None,
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(5_602),
        target_activity_advanced: true,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    let mut clock = super::SettledTargetEndpointClock::default();
    clock.note_visible_body_boundary(false, 34, started);
    let generation = clock
        .observe(&owner, true, started)
        .expect("settled owner arms endpoint");
    assert!(
        clock
            .due_update(
                generation,
                started + std::time::Duration::from_millis(900),
                900,
            )
            .is_none(),
        "recent uncertain tail still protects a pause"
    );

    let low_level_noise = crate::asr::volcengine::TargetSpeakerUpdate {
        provider_audio_duration_ms: Some(9_300),
        audio_duration_ms: Some(9_300),
        local_speech_end_ms: Some(9_300),
        target_activity_advanced: false,
        ..owner
    };
    assert_eq!(
        clock.observe(
            &low_level_noise,
            true,
            started + std::time::Duration::from_millis(2_100),
        ),
        None
    );
    assert!(
        clock
            .latest_due_update(started + std::time::Duration::from_millis(2_100), 900)
            .is_some(),
        "unclassified energy past the two-second ceiling cannot keep recording alive"
    );
}

#[test]
fn stale_uncertain_speaker_frame_cannot_hold_settled_owner_forever() {
    let started = std::time::Instant::now();
    let stale_uncertain_tail = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("0".into()),
        target_speech_end_ms: Some(5_032),
        provider_audio_duration_ms: Some(5_800),
        audio_duration_ms: Some(5_800),
        local_speech_end_ms: Some(5_800),
        qualified_owner_speech_end_ms: None,
        qualified_owner_activity_advanced: false,
        local_speaker_classification_kind: None,
        local_speaker_signal_quality_sufficient: None,
        local_speaker_observation_end_ms: None,
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: None,
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(5_032),
        target_activity_advanced: true,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    let mut clock = super::SettledTargetEndpointClock::default();
    clock.note_visible_body_boundary(false, 18, started);
    let generation = clock
        .observe(&stale_uncertain_tail, true, started)
        .expect("settled owner arms endpoint");

    assert!(
        clock
            .due_update(
                generation,
                started + std::time::Duration::from_millis(900),
                900,
            )
            .is_none(),
        "a fresh uncertain tail still gets its bounded owner-continuation window"
    );
    assert!(
        clock
            .latest_due_update(
                started
                    + std::time::Duration::from_millis(
                        super::EMBEDDED_UNRESOLVED_LOCAL_SPEECH_MAX_HOLD_MS
                            .max(super::EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS),
                    ),
                900,
            )
            .is_some(),
        "a stale provider snapshot must expire on wall time even without another speaker frame"
    );
}

#[test]
fn visible_body_never_lets_the_provider_clock_bypass_the_guarded_wall_clock() {
    assert!(!super::target_speaker_endpoint_due_after_visible_body_gate(
        true, true, false,
    ));
    assert!(super::target_speaker_endpoint_due_after_visible_body_gate(
        true, true, true,
    ));
    assert!(super::target_speaker_endpoint_due_after_visible_body_gate(
        false, true, false,
    ));
}

#[test]
fn settled_target_watchdog_survives_obsolete_timer_generation() {
    let started = std::time::Instant::now();
    let stable = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("0".into()),
        target_speech_end_ms: Some(6_002),
        provider_audio_duration_ms: Some(6_700),
        audio_duration_ms: Some(6_700),
        // This fixture exercises timer generations, not an active speech
        // tail. Keep local speech at the settled owner boundary so the
        // endpoint is genuinely due.
        local_speech_end_ms: Some(6_002),
        qualified_owner_speech_end_ms: None,
        qualified_owner_activity_advanced: false,
        local_speaker_classification_kind: None,
        local_speaker_signal_quality_sufficient: None,
        local_speaker_observation_end_ms: None,
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: None,
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(6_002),
        target_activity_advanced: true,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    let mut clock = super::SettledTargetEndpointClock::default();
    let obsolete_generation = clock
        .observe(&stable, true, started)
        .expect("first callback arms the clock");
    let active_generation =
        clock.arm_latest_for_visible_body(started + std::time::Duration::from_millis(1), true);
    assert_eq!(
        active_generation, None,
        "preview callback must not reset the wall clock"
    );

    let due_at = started + std::time::Duration::from_millis(1_001);
    assert!(clock
        .due_update(obsolete_generation, due_at, 1_000)
        .is_some());
    assert!(clock.latest_due_update(due_at, 1_000).is_none());
}

#[test]
fn repeated_preview_revisions_keep_original_endpoint_deadline() {
    let started = std::time::Instant::now();
    let stable = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("0".into()),
        target_speech_end_ms: Some(4_000),
        provider_audio_duration_ms: Some(4_100),
        audio_duration_ms: Some(4_100),
        local_speech_end_ms: Some(4_000),
        qualified_owner_speech_end_ms: Some(4_000),
        qualified_owner_activity_advanced: true,
        local_speaker_classification_kind: Some(crate::asr::volcengine::LocalSpeakerClassificationKind::Target),
        local_speaker_signal_quality_sufficient: Some(true),
        local_speaker_observation_end_ms: Some(4_000),
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: Some(4_000),
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(4_000),
        target_activity_advanced: true,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    let mut clock = super::SettledTargetEndpointClock::default();
    let generation = clock
        .observe(&stable, true, started)
        .expect("stable owner boundary arms endpoint");

    // Preview revisions are deliberately newer text, but not new owner audio
    // boundaries. They must not move the 900/1000 ms wall-clock deadline.
    for (revision, at_ms) in [(false, 100), (false, 300), (true, 500), (true, 700)] {
        let update = crate::asr::volcengine::TargetSpeakerUpdate {
            target_activity_advanced: false,
            pending_activity_advanced: false,
            ..stable.clone()
        };
        assert_eq!(
            clock.observe(
                &update,
                true,
                started + std::time::Duration::from_millis(at_ms)
            ),
            None,
            "preview revision {revision} must not re-arm endpoint",
        );
        assert_eq!(
            clock.arm_latest_for_visible_body(
                started + std::time::Duration::from_millis(at_ms),
                true,
            ),
            None,
        );
    }
    assert!(clock
        .due_update(
            generation,
            started + std::time::Duration::from_millis(1_000),
            1_000,
        )
        .is_some());
}

#[test]
fn visible_body_without_cloud_speaker_identity_still_ends_after_one_second() {
    let started = std::time::Instant::now();
    let unattributed = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: None,
        target_speech_end_ms: None,
        provider_audio_duration_ms: Some(2_100),
        audio_duration_ms: Some(2_300),
        local_speech_end_ms: Some(2_300),
        qualified_owner_speech_end_ms: None,
        qualified_owner_activity_advanced: false,
        local_speaker_classification_kind: None,
        local_speaker_signal_quality_sufficient: None,
        local_speaker_observation_end_ms: None,
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: None,
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: None,
        target_activity_advanced: false,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: false,
    };
    let mut clock = super::SettledTargetEndpointClock::default();
    assert_eq!(
        clock.observe(&unattributed, false, started),
        None,
        "speaker metadata alone must not arm before visible body text",
    );
    let generation = clock
        .arm_latest_for_visible_body(started, true)
        .expect("visible body arms the unattributed fallback");

    let noisy_room_update = crate::asr::volcengine::TargetSpeakerUpdate {
        provider_audio_duration_ms: Some(10_000),
        audio_duration_ms: Some(10_200),
        local_speech_end_ms: Some(10_200),
        ..unattributed
    };
    assert_eq!(
        clock.observe(
            &noisy_room_update,
            true,
            started + std::time::Duration::from_millis(999),
        ),
        None,
        "unattributed room energy must not postpone visible owner text forever",
    );
    assert!(clock
        .due_update(
            generation,
            started + std::time::Duration::from_millis(999),
            1_000,
        )
        .is_none());
    assert!(clock
        .latest_due_update(started + std::time::Duration::from_millis(1_000), 1_000)
        .is_some());
}

#[test]
fn strong_second_speaker_cannot_keep_rearming_visible_owner_text() {
    let started = std::time::Instant::now();
    let owner = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("0".into()),
        target_speech_end_ms: Some(3_000),
        provider_audio_duration_ms: Some(3_200),
        audio_duration_ms: Some(3_200),
        local_speech_end_ms: Some(3_200),
        qualified_owner_speech_end_ms: Some(3_000),
        qualified_owner_activity_advanced: true,
        local_speaker_classification_kind: Some(crate::asr::volcengine::LocalSpeakerClassificationKind::Target),
        local_speaker_signal_quality_sufficient: Some(true),
        local_speaker_observation_end_ms: Some(3_000),
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: Some(3_000),
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(3_000),
        target_activity_advanced: true,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    let mut clock = super::SettledTargetEndpointClock::default();
    let owner_generation = clock
        .observe(&owner, true, started)
        .expect("visible owner text arms the endpoint clock");

    // The provider incorrectly grows the same cloud speaker while two
    // consecutive strong local windows identify the current voice as someone
    // else. Neither the provider callback nor the preview callback may extend
    // the owner's one-second deadline.
    let mixed_room_growth = crate::asr::volcengine::TargetSpeakerUpdate {
        target_speech_end_ms: Some(3_700),
        provider_audio_duration_ms: Some(3_800),
        audio_duration_ms: Some(3_800),
        local_speech_end_ms: Some(3_800),
        local_non_target_speech_end_ms: Some(3_800),
        stable_attributed_speech_end_ms: Some(3_700),
        target_activity_advanced: true,
        ..owner
    };
    assert_eq!(
        clock.observe(
            &mixed_room_growth,
            true,
            started + std::time::Duration::from_millis(600),
        ),
        None,
    );
    assert_eq!(
        clock.arm_latest_for_visible_body(
            started + std::time::Duration::from_millis(700),
            true,
        ),
        None,
    );
    assert!(clock
        .due_update(
            owner_generation,
            started + std::time::Duration::from_millis(1_000),
            1_000,
        )
        .is_some());
}

#[test]
fn sustained_second_speaker_can_end_while_cloud_tail_stays_provisional() {
    // Installed session 752: the owner body was already visible, then a nearby
    // second person kept the provider's unattributed tail growing for 4.8 s.
    // Sustained endpoint-grade owner absence must keep that provisional cloud
    // tail from cancelling the owner-only wall clock.
    let started = std::time::Instant::now();
    let owner = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("0".into()),
        target_speech_end_ms: Some(3_000),
        provider_audio_duration_ms: Some(3_200),
        audio_duration_ms: Some(3_200),
        local_speech_end_ms: Some(3_200),
        qualified_owner_speech_end_ms: Some(3_000),
        qualified_owner_activity_advanced: true,
        local_speaker_classification_kind: Some(crate::asr::volcengine::LocalSpeakerClassificationKind::Target),
        local_speaker_signal_quality_sufficient: Some(true),
        local_speaker_observation_end_ms: Some(3_000),
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: Some(3_000),
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(3_000),
        target_activity_advanced: true,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    let pending_other = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: None,
        target_speech_end_ms: None,
        provider_audio_duration_ms: Some(3_900),
        audio_duration_ms: Some(4_000),
        local_speech_end_ms: Some(4_000),
        qualified_owner_speech_end_ms: None,
        qualified_owner_activity_advanced: false,
        local_speaker_classification_kind: None,
        local_speaker_signal_quality_sufficient: None,
        local_speaker_observation_end_ms: None,
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: None,
        local_non_target_speech_end_ms: Some(3_900),
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: None,
        target_activity_advanced: false,
        pending_unattributed_speech: true,
        pending_activity_advanced: true,
        speaker_info_present: false,
    };
    let mut clock = super::SettledTargetEndpointClock::default();
    let owner_generation = clock
        .observe(&owner, true, started)
        .expect("visible owner text arms the endpoint clock");

    let provisional_other = crate::asr::volcengine::TargetSpeakerUpdate {
        local_non_target_speech_end_ms: None,
        ..pending_other.clone()
    };
    assert_eq!(
        clock.observe(
            &provisional_other,
            true,
            started + std::time::Duration::from_millis(200),
        ),
        None,
        "unresolved provisional speech pauses the owner deadline"
    );
    assert!(clock
        .due_update(
            owner_generation,
            started + std::time::Duration::from_millis(900),
            900,
        )
        .is_none());

    let restored_generation = clock
        .observe(
            &pending_other,
            true,
            started + std::time::Duration::from_millis(800),
        )
        .expect("sustained other speech restores the original owner deadline");

    let continuing_other = crate::asr::volcengine::TargetSpeakerUpdate {
        provider_audio_duration_ms: Some(4_700),
        audio_duration_ms: Some(4_800),
        local_speech_end_ms: Some(4_800),
        local_non_target_speech_end_ms: Some(4_800),
        ..pending_other
    };
    assert_eq!(
        clock.observe(
            &continuing_other,
            true,
            started + std::time::Duration::from_millis(850),
        ),
        None,
        "continuing room speech must not re-arm the owner deadline"
    );
    assert!(clock
        .due_update(
            restored_generation,
            started + std::time::Duration::from_millis(900),
            900,
        )
        .is_some());
}

#[test]
fn collapsed_cloud_speaker_id_cannot_hold_provisional_tail_after_local_non_target() {
    // Cloud diarization can keep assigning the interfering voice to the
    // owner's speaker id.  Local identity is still authoritative for the
    // endpoint: a provisional tail must not block stop merely because the
    // provider never emits a distinct `stable_attributed` boundary.
    let update = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("0".into()),
        target_speech_end_ms: Some(3_000),
        provider_audio_duration_ms: Some(6_000),
        audio_duration_ms: Some(6_000),
        local_speech_end_ms: Some(4_200),
        qualified_owner_speech_end_ms: Some(3_000),
        qualified_owner_activity_advanced: true,
        local_speaker_classification_kind: Some(crate::asr::volcengine::LocalSpeakerClassificationKind::Target),
        local_speaker_signal_quality_sufficient: Some(true),
        local_speaker_observation_end_ms: Some(3_000),
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: Some(3_000),
        local_non_target_speech_end_ms: Some(4_200),
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(3_000),
        target_activity_advanced: false,
        pending_unattributed_speech: true,
        pending_activity_advanced: true,
        speaker_info_present: true,
    };
    assert!(super::target_speaker_endpoint_due_with_provider_stall(
        &update,
        false,
        super::EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS,
    ));
}

#[test]
fn settled_target_wall_clock_cancels_for_provisional_tail_and_rearms_when_stable() {
    let started = std::time::Instant::now();
    let stable = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("1".into()),
        target_speech_end_ms: Some(4_900),
        provider_audio_duration_ms: Some(5_200),
        audio_duration_ms: Some(5_300),
        local_speech_end_ms: Some(5_200),
        qualified_owner_speech_end_ms: Some(5_200),
        qualified_owner_activity_advanced: true,
        local_speaker_classification_kind: Some(crate::asr::volcengine::LocalSpeakerClassificationKind::Target),
        local_speaker_signal_quality_sufficient: Some(true),
        local_speaker_observation_end_ms: Some(5_200),
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: Some(5_200),
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(4_900),
        target_activity_advanced: true,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    let mut clock = super::SettledTargetEndpointClock::default();
    let first_generation = clock
        .observe(&stable, true, started)
        .expect("first stable boundary arms");

    let pending = crate::asr::volcengine::TargetSpeakerUpdate {
        pending_unattributed_speech: true,
        pending_activity_advanced: true,
        target_activity_advanced: false,
        audio_duration_ms: Some(5_900),
        local_speech_end_ms: Some(5_900),
        ..stable.clone()
    };
    assert!(clock
        .observe(
            &pending,
            true,
            started + std::time::Duration::from_millis(700),
        )
        .is_none());
    assert!(clock
        .due_update(
            first_generation,
            started + std::time::Duration::from_millis(1_100),
            1_000,
        )
        .is_none());

    let final_stable = crate::asr::volcengine::TargetSpeakerUpdate {
        target_speech_end_ms: Some(6_300),
        provider_audio_duration_ms: Some(6_700),
        audio_duration_ms: Some(6_800),
        local_speech_end_ms: Some(6_300),
        local_target_speech_end_ms: Some(6_300),
        stable_attributed_speech_end_ms: Some(6_300),
        target_activity_advanced: true,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        ..stable
    };
    let settled_at = started + std::time::Duration::from_millis(1_200);
    let final_generation = clock
        .observe(&final_stable, true, settled_at)
        .expect("final stable boundary rearms from its own publication time");
    assert!(clock
        .due_update(
            final_generation,
            settled_at + std::time::Duration::from_millis(2_499),
            1_000,
        )
        .is_none());
    assert!(clock
        .due_update(
            final_generation,
            settled_at + std::time::Duration::from_millis(2_500),
            1_000,
        )
        .is_some());
}

#[test]
fn target_speaker_endpoint_requires_one_second_without_that_speaker() {
    let update = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("1".into()),
        target_speech_end_ms: Some(1_500),
        provider_audio_duration_ms: Some(2_499),
        audio_duration_ms: Some(2_499),
        local_speech_end_ms: Some(1_500),
        qualified_owner_speech_end_ms: None,
        qualified_owner_activity_advanced: false,
        local_speaker_classification_kind: None,
        local_speaker_signal_quality_sufficient: None,
        local_speaker_observation_end_ms: None,
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: None,
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: false,
        stable_attributed_speech_end_ms: Some(1_500),
        target_activity_advanced: false,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    assert!(!super::target_speaker_endpoint_due(&update));

    let due = crate::asr::volcengine::TargetSpeakerUpdate {
        provider_audio_duration_ms: Some(4_500),
        audio_duration_ms: Some(4_500),
        ..update.clone()
    };
    assert!(super::target_speaker_endpoint_due(&due));

    let pending = crate::asr::volcengine::TargetSpeakerUpdate {
        pending_unattributed_speech: true,
        pending_activity_advanced: true,
        ..due.clone()
    };
    // Pending provider text without a current owner watermark cannot block
    // an owner-tracked endpoint; final text is handled after capture stops.
    assert!(super::target_speaker_endpoint_due(&pending));

    let unresolved_recent_local = crate::asr::volcengine::TargetSpeakerUpdate {
        audio_duration_ms: Some(3_100),
        local_speech_end_ms: Some(3_000),
        local_target_speech_end_ms: Some(2_500),
        stable_attributed_speech_end_ms: Some(1_500),
        ..due.clone()
    };
    assert!(!super::target_speaker_endpoint_due(
        &unresolved_recent_local
    ));

    // Confirmed other-speaker energy does not refresh the owner clock: once the
    // owner has been inactive for 1000 ms, auto-end proceeds while others talk.
    let unresolved_local_is_confidently_other_speaker =
        crate::asr::volcengine::TargetSpeakerUpdate {
            local_non_target_speech_end_ms: Some(3_000),
            local_target_speech_end_ms: Some(1_500),
            ..unresolved_recent_local.clone()
        };
    assert!(super::target_speaker_endpoint_due(
        &unresolved_local_is_confidently_other_speaker
    ));

    let stale_non_target_classification = crate::asr::volcengine::TargetSpeakerUpdate {
        local_non_target_speech_end_ms: Some(2_399),
        ..unresolved_recent_local.clone()
    };
    assert!(!super::target_speaker_endpoint_due(
        &stale_non_target_classification
    ));

    let unresolved_local_has_reached_its_own_one_second_endpoint =
        crate::asr::volcengine::TargetSpeakerUpdate {
            audio_duration_ms: Some(6_000),
            provider_audio_duration_ms: Some(6_000),
            ..unresolved_recent_local
        };
    assert!(super::target_speaker_endpoint_due(
        &unresolved_local_has_reached_its_own_one_second_endpoint
    ));

    let no_identity = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_info_present: false,
        ..due
    };
    assert!(!super::target_speaker_endpoint_due(&no_identity));

    let local_wake_target = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: None,
        target_speech_end_ms: None,
        provider_audio_duration_ms: None,
        audio_duration_ms: Some(2_499),
        local_speech_end_ms: Some(1_500),
        qualified_owner_speech_end_ms: Some(1_500),
        qualified_owner_activity_advanced: true,
        local_speaker_classification_kind: Some(crate::asr::volcengine::LocalSpeakerClassificationKind::Target),
        local_speaker_signal_quality_sufficient: Some(true),
        local_speaker_observation_end_ms: Some(1_500),
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: Some(1_500),
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: None,
        target_activity_advanced: false,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: false,
    };
    assert!(!super::target_speaker_endpoint_due(&local_wake_target));
    // Pending provisional body without a current positive owner watermark must
    // not postpone the owner's one-second endpoint.
    let local_wake_still_pending = crate::asr::volcengine::TargetSpeakerUpdate {
        audio_duration_ms: Some(4_500),
        pending_unattributed_speech: true,
        ..local_wake_target.clone()
    };
    assert!(super::target_speaker_endpoint_due(
        &local_wake_still_pending
    ));
    let local_wake_due = crate::asr::volcengine::TargetSpeakerUpdate {
        audio_duration_ms: Some(4_500),
        pending_unattributed_speech: false,
        ..local_wake_target
    };
    assert!(super::target_speaker_endpoint_due(&local_wake_due));
}

#[test]
fn target_speaker_endpoint_uses_newest_stable_attributed_boundary_after_diarization_flip() {
    // Installed session 909d8f82 reproduced a same-owner diarization flip:
    // target speaker stopped at 13772 ms, while the provider had already
    // stabilized a newer spoken tail through 15742 ms. Stopping at provider
    // audio 16300 ms therefore waited only 558 ms and truncated the owner.
    // Newest protection clock is stable_attributed 15742 ms; exact due is +1000.
    let one_ms_before = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("0".into()),
        target_speech_end_ms: Some(13_772),
        provider_audio_duration_ms: Some(16_741),
        audio_duration_ms: Some(16_800),
        local_speech_end_ms: Some(15_742),
        qualified_owner_speech_end_ms: Some(15_200),
        qualified_owner_activity_advanced: true,
        local_speaker_classification_kind: Some(crate::asr::volcengine::LocalSpeakerClassificationKind::Target),
        local_speaker_signal_quality_sufficient: Some(true),
        local_speaker_observation_end_ms: Some(15_200),
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: Some(15_200),
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(15_742),
        target_activity_advanced: false,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    assert!(!super::target_speaker_endpoint_due(&one_ms_before));

    let exact_endpoint = crate::asr::volcengine::TargetSpeakerUpdate {
        provider_audio_duration_ms: Some(18_742),
        audio_duration_ms: Some(18_742),
        ..one_ms_before
    };
    assert!(super::target_speaker_endpoint_due(&exact_endpoint));
}

#[test]
fn confirmed_other_speaker_does_not_extend_endpoint_via_provider_attribution() {
    // Installed session 245: the owner ended at 10842 ms. A nearby speaker then
    // advanced stable attribution to 15132 ms and kept a provisional tail open.
    // Repeated strong local NonTarget evidence must keep both provider channels
    // from extending the owner's endpoint clock.
    let update = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("0".into()),
        target_speech_end_ms: Some(10_842),
        provider_audio_duration_ms: Some(15_600),
        audio_duration_ms: Some(15_700),
        local_speech_end_ms: Some(14_700),
        qualified_owner_speech_end_ms: None,
        qualified_owner_activity_advanced: false,
        local_speaker_classification_kind: None,
        local_speaker_signal_quality_sufficient: None,
        local_speaker_observation_end_ms: None,
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: None,
        local_non_target_speech_end_ms: Some(14_600),
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(15_132),
        target_activity_advanced: false,
        pending_unattributed_speech: true,
        pending_activity_advanced: true,
        speaker_info_present: true,
    };

    assert!(super::target_speaker_endpoint_due(&update));
    let without_local_other = crate::asr::volcengine::TargetSpeakerUpdate {
        local_non_target_speech_end_ms: None,
        ..update
    };
    assert!(!super::target_speaker_endpoint_due(&without_local_other));
}

#[test]
fn target_speaker_endpoint_waits_for_provider_coverage_before_stopping_quiet_tail() {
    let provider_is_behind = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("0".into()),
        target_speech_end_ms: Some(5_112),
        provider_audio_duration_ms: Some(6_200),
        audio_duration_ms: Some(6_900),
        local_speech_end_ms: Some(5_900),
        qualified_owner_speech_end_ms: Some(5_700),
        qualified_owner_activity_advanced: true,
        local_speaker_classification_kind: Some(crate::asr::volcengine::LocalSpeakerClassificationKind::Target),
        local_speaker_signal_quality_sufficient: Some(true),
        local_speaker_observation_end_ms: Some(5_700),
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: Some(5_700),
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(5_112),
        target_activity_advanced: false,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    assert!(!super::target_speaker_endpoint_due(&provider_is_behind));

    let quiet_tail_arrives = crate::asr::volcengine::TargetSpeakerUpdate {
        target_speech_end_ms: Some(6_442),
        provider_audio_duration_ms: Some(6_900),
        stable_attributed_speech_end_ms: Some(6_442),
        target_activity_advanced: true,
        ..provider_is_behind.clone()
    };
    assert!(!super::target_speaker_endpoint_due(&quiet_tail_arrives));

    let one_ms_before_exact_endpoint = crate::asr::volcengine::TargetSpeakerUpdate {
        provider_audio_duration_ms: Some(9_441),
        audio_duration_ms: Some(8_899),
        ..quiet_tail_arrives.clone()
    };
    assert!(!super::target_speaker_endpoint_due(
        &one_ms_before_exact_endpoint
    ));

    let exact_endpoint = crate::asr::volcengine::TargetSpeakerUpdate {
        provider_audio_duration_ms: Some(9_442),
        audio_duration_ms: Some(8_900),
        ..one_ms_before_exact_endpoint
    };
    assert!(super::target_speaker_endpoint_due(&exact_endpoint));
}

#[test]
fn generic_energy_cannot_extend_an_owner_tracked_endpoint() {
    let update = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("owner".into()),
        target_speech_end_ms: Some(4_000),
        provider_audio_duration_ms: Some(7_000),
        audio_duration_ms: Some(7_000),
        // The generic energy detector still sees room activity at the live
        // edge, but the owner watermark stopped at 4 s.
        local_speech_end_ms: Some(7_000),
        qualified_owner_speech_end_ms: Some(4_000),
        qualified_owner_activity_advanced: true,
        local_speaker_classification_kind: Some(crate::asr::volcengine::LocalSpeakerClassificationKind::Target),
        local_speaker_signal_quality_sufficient: Some(true),
        local_speaker_observation_end_ms: Some(4_000),
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: Some(4_000),
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(4_000),
        target_activity_advanced: false,
        pending_unattributed_speech: true,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    assert!(super::target_speaker_endpoint_due(&update));
    let mut clock = super::SettledTargetEndpointClock::default();
    assert!(!clock.should_renew_firmware_endpoint_lease(&update, true));
}

#[test]
fn target_speaker_endpoint_ignores_late_cloud_boundary_without_activity_edge() {
    let started = std::time::Instant::now();
    let initial = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("owner".into()),
        target_speech_end_ms: Some(7_000),
        provider_audio_duration_ms: Some(7_400),
        audio_duration_ms: Some(7_400),
        local_speech_end_ms: Some(7_000),
        qualified_owner_speech_end_ms: Some(7_000),
        qualified_owner_activity_advanced: true,
        local_speaker_classification_kind: Some(crate::asr::volcengine::LocalSpeakerClassificationKind::Target),
        local_speaker_signal_quality_sufficient: Some(true),
        local_speaker_observation_end_ms: Some(7_000),
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: Some(7_000),
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(7_000),
        target_activity_advanced: true,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    let mut clock = super::SettledTargetEndpointClock::default();
    let generation = clock
        .observe(&initial, true, started)
        .expect("initial owner evidence arms endpoint");

    // The provider publishes a delayed diarization row for audio that was
    // already captured. It advances the cloud boundary but carries no fresh
    // owner-activity edge; it must not restart the one-second wall clock.
    let late = crate::asr::volcengine::TargetSpeakerUpdate {
        target_speech_end_ms: Some(12_000),
        stable_attributed_speech_end_ms: Some(12_000),
        provider_audio_duration_ms: Some(12_200),
        audio_duration_ms: Some(12_200),
        local_speech_end_ms: Some(10_000),
        qualified_owner_activity_advanced: false,
        target_activity_advanced: false,
        pending_activity_advanced: false,
        ..initial
    };
    assert_eq!(
        clock.observe(&late, true, started + std::time::Duration::from_millis(900),),
        None,
        "late cloud attribution must not rearm a due endpoint"
    );
    assert!(clock
        .due_update(
            generation,
            started + std::time::Duration::from_millis(1_000),
            1_000,
        )
        .is_some());
}

#[test]
fn target_speaker_endpoint_uses_local_clock_only_for_a_clean_provider_stall() {
    let one_ms_before = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("0".into()),
        target_speech_end_ms: Some(9_992),
        provider_audio_duration_ms: Some(10_400),
        audio_duration_ms: Some(10_991),
        local_speech_end_ms: Some(9_400),
        qualified_owner_speech_end_ms: Some(9_400),
        qualified_owner_activity_advanced: true,
        local_speaker_classification_kind: Some(crate::asr::volcengine::LocalSpeakerClassificationKind::Target),
        local_speaker_signal_quality_sufficient: Some(true),
        local_speaker_observation_end_ms: Some(9_400),
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: Some(9_400),
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(9_992),
        target_activity_advanced: false,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    assert!(!super::provider_stall_local_endpoint_due(
        &one_ms_before,
        true,
        1_000,
    ));
    assert!(!super::target_speaker_endpoint_due_with_provider_stall(
        &one_ms_before,
        true,
        1_000,
    ));

    let exact_endpoint = crate::asr::volcengine::TargetSpeakerUpdate {
        audio_duration_ms: Some(10_992),
        ..one_ms_before.clone()
    };
    assert!(super::provider_stall_local_endpoint_due(
        &exact_endpoint,
        true,
        1_000,
    ));
    assert!(super::target_speaker_endpoint_due_with_provider_stall(
        &exact_endpoint,
        true,
        1_000,
    ));

    let ordinary_provider_lag = crate::asr::volcengine::TargetSpeakerUpdate {
        provider_audio_duration_ms: Some(10_493),
        ..exact_endpoint.clone()
    };
    assert!(!super::provider_stall_local_endpoint_due(
        &ordinary_provider_lag,
        true,
        1_000,
    ));
    assert!(!super::target_speaker_endpoint_due_with_provider_stall(
        &ordinary_provider_lag,
        true,
        1_000,
    ));

    // A stale provisional row with no unresolved local owner tail must not
    // disable the provider-stall fallback: this is the interference case
    // where the cloud keeps `pending` set after the room has gone quiet.
    let pending_tail = crate::asr::volcengine::TargetSpeakerUpdate {
        pending_unattributed_speech: true,
        ..exact_endpoint.clone()
    };
    assert!(super::provider_stall_local_endpoint_due(
        &pending_tail,
        true,
        1_000,
    ));
    assert!(super::target_speaker_endpoint_due_with_provider_stall(
        &pending_tail,
        true,
        1_000,
    ));

    // Pending text plus a fresh, owner-confirmed local tail remains a hard
    // hold so a cloud stall cannot cut a sentence in half. Generic energy
    // alone is deliberately not enough.
    let pending_owner_tail = crate::asr::volcengine::TargetSpeakerUpdate {
        pending_unattributed_speech: true,
        local_speech_end_ms: Some(10_400),
        local_target_speech_end_ms: Some(10_400),
        audio_duration_ms: Some(10_400),
        ..exact_endpoint.clone()
    };
    assert!(!super::provider_stall_local_endpoint_due(
        &pending_owner_tail,
        true,
        1_000,
    ));
    assert!(!super::target_speaker_endpoint_due_with_provider_stall(
        &pending_owner_tail,
        true,
        1_000,
    ));

    // Fresh speech near an established owner gets a bounded classification
    // grace. It is not silence merely because provider coverage stalled.
    let mid_sentence_local_energy = crate::asr::volcengine::TargetSpeakerUpdate {
        local_speech_end_ms: Some(10_400),
        ..exact_endpoint.clone()
    };
    assert!(!super::provider_stall_local_endpoint_due(
        &mid_sentence_local_energy,
        true,
        1_000,
    ));
    assert!(!super::target_speaker_endpoint_due_with_provider_stall(
        &mid_sentence_local_energy,
        true,
        1_000,
    ));

    // Residual energy that has itself been quiet for the full endpoint interval
    // may still use stall fallback so room noise does not hold the session open.
    let residual_energy_now_quiet = crate::asr::volcengine::TargetSpeakerUpdate {
        local_speech_end_ms: Some(9_900),
        ..exact_endpoint.clone()
    };
    assert!(super::provider_stall_local_endpoint_due(
        &residual_energy_now_quiet,
        true,
        1_000,
    ));
    assert!(super::target_speaker_endpoint_due_with_provider_stall(
        &residual_energy_now_quiet,
        true,
        1_000,
    ));

    let confirmed_other_speaker = crate::asr::volcengine::TargetSpeakerUpdate {
        audio_duration_ms: Some(11_000),
        local_speech_end_ms: Some(10_500),
        local_non_target_speech_end_ms: Some(10_500),
        ..exact_endpoint.clone()
    };
    assert!(super::provider_stall_local_endpoint_due(
        &confirmed_other_speaker,
        true,
        1_000,
    ));
    assert!(super::target_speaker_endpoint_due_with_provider_stall(
        &confirmed_other_speaker,
        true,
        1_000,
    ));

    // Installed session 1026: cloud had already established the owner, the
    // provider then stalled, and repeated local windows confirmed that the
    // continuing room voice was somebody else. There was no local Target vote,
    // so the old fallback could never auto-end and recording hung until click.
    let confirmed_other_without_local_target = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("1".into()),
        target_speech_end_ms: Some(9_042),
        provider_audio_duration_ms: Some(9_500),
        audio_duration_ms: Some(13_400),
        local_speech_end_ms: Some(13_400),
        qualified_owner_speech_end_ms: None,
        qualified_owner_activity_advanced: false,
        local_speaker_classification_kind: None,
        local_speaker_signal_quality_sufficient: None,
        local_speaker_observation_end_ms: None,
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: None,
        local_non_target_speech_end_ms: Some(13_400),
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(9_042),
        target_activity_advanced: false,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    assert!(super::provider_stall_local_endpoint_due(
        &confirmed_other_without_local_target,
        true,
        1_000,
    ));
    assert!(super::target_speaker_endpoint_due_with_provider_stall(
        &confirmed_other_without_local_target,
        true,
        1_000,
    ));

    // Installed session 1831: the provider froze at 8.9 s after the owner had
    // ended at 7.4 s. The overlapping local verifier confirmed the continuing
    // room speaker at 13.9 s while the newer VAD edge was already about 14.3 s.
    // One verifier cadence of measurement lag must still count as confirmed
    // other-speaker activity, otherwise the stalled cloud clock holds recording
    // open until the user clicks stop.
    let installed_interferer_with_classifier_lag = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("0".into()),
        target_speech_end_ms: Some(7_402),
        provider_audio_duration_ms: Some(8_900),
        audio_duration_ms: Some(14_300),
        local_speech_end_ms: Some(14_300),
        qualified_owner_speech_end_ms: None,
        qualified_owner_activity_advanced: false,
        local_speaker_classification_kind: None,
        local_speaker_signal_quality_sufficient: None,
        local_speaker_observation_end_ms: None,
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: None,
        local_non_target_speech_end_ms: Some(13_900),
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(8_562),
        target_activity_advanced: false,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    assert!(super::provider_stall_local_endpoint_due(
        &installed_interferer_with_classifier_lag,
        true,
        1_000,
    ));
    assert!(super::target_speaker_endpoint_due_with_provider_stall(
        &installed_interferer_with_classifier_lag,
        true,
        1_000,
    ));

    let unclassified_owner_may_still_be_talking = crate::asr::volcengine::TargetSpeakerUpdate {
        local_non_target_speech_end_ms: None,
        ..confirmed_other_without_local_target
    };
    assert!(
        !super::provider_stall_local_endpoint_due(
            &unclassified_owner_may_still_be_talking,
            true,
            1_000,
        ),
        "cloud stalls must not cut ongoing unclassified owner speech"
    );

    let newer_local_target_one_ms_before = crate::asr::volcengine::TargetSpeakerUpdate {
        target_speech_end_ms: Some(4_572),
        provider_audio_duration_ms: Some(5_200),
        audio_duration_ms: Some(5_899),
        local_speech_end_ms: Some(4_900),
        qualified_owner_speech_end_ms: Some(4_900),
        local_speaker_observation_end_ms: Some(4_900),
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: Some(4_900),
        stable_attributed_speech_end_ms: Some(4_572),
        ..exact_endpoint.clone()
    };
    assert!(!super::provider_stall_local_endpoint_due(
        &newer_local_target_one_ms_before,
        true,
        1_000,
    ));
    assert!(!super::target_speaker_endpoint_due_with_provider_stall(
        &newer_local_target_one_ms_before,
        true,
        1_000,
    ));

    let newer_local_target_exact_endpoint = crate::asr::volcengine::TargetSpeakerUpdate {
        audio_duration_ms: Some(5_900),
        ..newer_local_target_one_ms_before
    };
    assert!(super::provider_stall_local_endpoint_due(
        &newer_local_target_exact_endpoint,
        true,
        1_000,
    ));
    assert!(super::target_speaker_endpoint_due_with_provider_stall(
        &newer_local_target_exact_endpoint,
        true,
        1_000,
    ));

    // Stalled provider with newer local target: cloud 4572, local target 4900,
    // provider frozen at 5700. Capture must reach local_target + 1000 ms.
    let installed_newer_target_stall = crate::asr::volcengine::TargetSpeakerUpdate {
        target_speech_end_ms: Some(4_572),
        provider_audio_duration_ms: Some(5_700),
        audio_duration_ms: Some(6_200),
        local_speech_end_ms: Some(5_200),
        qualified_owner_speech_end_ms: Some(4_900),
        local_speaker_observation_end_ms: Some(4_900),
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: Some(4_900),
        stable_attributed_speech_end_ms: Some(4_572),
        ..exact_endpoint
    };
    assert!(super::provider_stall_local_endpoint_due(
        &installed_newer_target_stall,
        true,
        1_000,
    ));
    assert!(super::target_speaker_endpoint_due_with_provider_stall(
        &installed_newer_target_stall,
        true,
        1_000,
    ));
}

#[test]
fn owner_identity_uncertainty_does_not_slow_the_one_second_endpoint() {
    // Installed session 1378: the owner was locally confirmed through 2100 ms,
    // later speech energy reached 3100 ms with only Uncertain classifications,
    // and the provider still attributed the same speaker through 3612 ms. The
    // old 1.0.5 policy extended identity uncertainty to 2000 ms. The owner
    // restored the product contract to one second for every body ending; the
    // speaker state remains diagnostic and still protects attribution.
    let uncertain_tail = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("0".into()),
        target_speech_end_ms: Some(3_612),
        provider_audio_duration_ms: Some(4_100),
        audio_duration_ms: Some(6_400),
        local_speech_end_ms: Some(3_100),
        qualified_owner_speech_end_ms: Some(2_100),
        qualified_owner_activity_advanced: true,
        local_speaker_classification_kind: Some(crate::asr::volcengine::LocalSpeakerClassificationKind::Target),
        local_speaker_signal_quality_sufficient: Some(true),
        local_speaker_observation_end_ms: Some(2_100),
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: Some(2_100),
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(3_612),
        target_activity_advanced: false,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    assert_eq!(
        super::target_speaker_fusion_state(&uncertain_tail),
        super::TargetSpeakerFusionState::Quiet,
    );
    let timeout = super::target_speaker_endpoint_timeout_with_fusion(
        super::target_speaker_fusion_state(&uncertain_tail),
        1_000,
    );
    assert_eq!(timeout, 1_000);
    assert!(super::target_speaker_endpoint_due_with_provider_stall(
        &uncertain_tail,
        true,
        timeout,
    ));

    let bounded_due = crate::asr::volcengine::TargetSpeakerUpdate {
        audio_duration_ms: Some(6_112),
        ..uncertain_tail.clone()
    };
    assert!(super::target_speaker_endpoint_due_with_provider_stall(
        &bounded_due,
        true,
        timeout,
    ));

    let confirmed_other = crate::asr::volcengine::TargetSpeakerUpdate {
        local_non_target_speech_end_ms: Some(3_100),
        ..uncertain_tail
    };
    assert_eq!(
        super::target_speaker_fusion_state(&confirmed_other),
        super::TargetSpeakerFusionState::ConfirmedOther,
    );
    assert_eq!(
        super::target_speaker_endpoint_timeout_with_fusion(
            super::target_speaker_fusion_state(&confirmed_other),
            1_000,
        ),
        1_000,
    );
    assert!(super::target_speaker_endpoint_due_with_provider_stall(
        &confirmed_other,
        true,
        1_000,
    ));

    let provider_owner_advanced = crate::asr::volcengine::TargetSpeakerUpdate {
        target_activity_advanced: true,
        ..bounded_due
    };
    assert_eq!(
        super::target_speaker_fusion_state(&provider_owner_advanced),
        super::TargetSpeakerFusionState::Quiet,
        "cloud progress without a fresh aligned owner edge is quiet",
    );
    let quiet_update = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("0".into()),
        target_speech_end_ms: Some(1_000),
        provider_audio_duration_ms: Some(4_000),
        audio_duration_ms: Some(4_000),
        local_speech_end_ms: Some(1_000),
        qualified_owner_speech_end_ms: Some(1_000),
        qualified_owner_activity_advanced: true,
        local_speaker_classification_kind: Some(crate::asr::volcengine::LocalSpeakerClassificationKind::Target),
        local_speaker_signal_quality_sufficient: Some(true),
        local_speaker_observation_end_ms: Some(1_000),
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: Some(1_000),
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(1_000),
        target_activity_advanced: false,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    let speaking_update = crate::asr::volcengine::TargetSpeakerUpdate {
        audio_duration_ms: Some(4_000),
        local_speech_end_ms: Some(3_800),
        local_target_speech_end_ms: Some(3_800),
        ..quiet_update.clone()
    };
    assert!(super::owner_endpoint_stop_blocked_by_live_owner(
        super::TargetSpeakerFusionState::OwnerContinuing,
        &quiet_update,
        true,
        false,
            false));
    assert!(super::owner_endpoint_stop_blocked_by_live_owner(
        super::TargetSpeakerFusionState::UncertainOwnerTail,
        &quiet_update,
        true,
        false,
            false));
    assert!(
        super::owner_endpoint_stop_blocked_by_live_owner(
            super::TargetSpeakerFusionState::Quiet,
            &speaking_update,
            false,
            false,
            false),
        "fresh local speech must block no-body stop even if fusion is still Quiet"
    );
    assert!(
        !super::owner_endpoint_stop_blocked_by_live_owner(
            super::TargetSpeakerFusionState::Quiet,
            &speaking_update,
            true,
            false,
            false),
        "after body text exists, Quiet without recent preview growth must auto-end"
    );
    assert!(
        !super::owner_endpoint_stop_blocked_by_live_owner(
            super::TargetSpeakerFusionState::Quiet,
            &quiet_update,
            true,
            true,
            false),
        "ordinary preview growth must not reopen a quiet owner endpoint"
    );
    let fresh_owner_preview = crate::asr::volcengine::TargetSpeakerUpdate {
        target_speech_end_ms: Some(3_800),
        stable_attributed_speech_end_ms: Some(3_800),
        target_activity_advanced: true,
        ..speaking_update.clone()
    };
    assert!(
        super::owner_endpoint_stop_blocked_by_live_owner(
            super::TargetSpeakerFusionState::Quiet,
            &fresh_owner_preview,
            true,
            true,
            false),
        "fresh owner-aligned preview growth must still hold the endpoint"
    );
    assert!(!super::owner_endpoint_stop_blocked_by_live_owner(
        super::TargetSpeakerFusionState::ConfirmedOther,
        &speaking_update,
        false,
        true,
            false));
}

#[test]
fn confirmed_other_stop_deferred_while_visible_body_text_grows() {
    // ef-hybrid-r1 2026-09-18: continuous TTS interference made the local
    // classifier read ConfirmedOther while the real owner was mid-body and
    // the visible, target-attributed preview kept growing; the 1000 ms
    // target-inactive stop cut a ~15 s body in half at 7.9 s. A
    // ConfirmedOther stop on a started body must defer while the visible
    // text still grows, and resume once the growth settles.
    let confirmed_other = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("0".into()),
        target_speech_end_ms: Some(3_612),
        provider_audio_duration_ms: Some(4_100),
        audio_duration_ms: Some(6_400),
        local_speech_end_ms: Some(3_100),
        qualified_owner_speech_end_ms: Some(2_100),
        qualified_owner_activity_advanced: true,
        local_speaker_classification_kind: Some(crate::asr::volcengine::LocalSpeakerClassificationKind::NonTarget),
        local_speaker_signal_quality_sufficient: Some(true),
        local_speaker_observation_end_ms: Some(3_100),
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: Some(2_100),
        local_non_target_speech_end_ms: Some(3_100),
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(3_612),
        target_activity_advanced: false,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    assert_eq!(
        super::target_speaker_fusion_state(&confirmed_other),
        super::TargetSpeakerFusionState::ConfirmedOther,
    );
    assert!(
        super::owner_endpoint_stop_blocked_by_live_owner(
            super::TargetSpeakerFusionState::ConfirmedOther,
            &confirmed_other,
            true,
            true,
            false),
        "confirmed-other must not cut a started body whose visible text is still growing"
    );
    assert!(
        !super::owner_endpoint_stop_blocked_by_live_owner(
            super::TargetSpeakerFusionState::ConfirmedOther,
            &confirmed_other,
            true,
            false,
            false),
        "confirmed-other may stop once the visible preview growth has settled"
    );
    assert!(
        !super::owner_endpoint_stop_blocked_by_live_owner(
            super::TargetSpeakerFusionState::ConfirmedOther,
            &confirmed_other,
            false,
            true,
            false),
        "no-body sessions keep the immediate ConfirmedOther stop (G stays lower priority)"
    );
}

#[test]
fn provider_stall_requires_real_time_without_provider_coverage_progress() {
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    let started = Instant::now();
    let update = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("0".into()),
        target_speech_end_ms: Some(4_572),
        provider_audio_duration_ms: Some(5_700),
        audio_duration_ms: Some(6_200),
        local_speech_end_ms: Some(5_200),
        qualified_owner_speech_end_ms: Some(4_900),
        qualified_owner_activity_advanced: true,
        local_speaker_classification_kind: Some(crate::asr::volcengine::LocalSpeakerClassificationKind::Target),
        local_speaker_signal_quality_sufficient: Some(true),
        local_speaker_observation_end_ms: Some(4_900),
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: Some(4_900),
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(4_572),
        target_activity_advanced: false,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };

    assert!(!super::provider_progress_stalled(
        &coordinator.inner,
        session_id,
        &update,
        started
    ));
    assert!(!super::provider_progress_stalled(
        &coordinator.inner,
        session_id,
        &update,
        started + Duration::from_millis(999)
    ));
    assert!(super::provider_progress_stalled(
        &coordinator.inner,
        session_id,
        &update,
        started + Duration::from_millis(1_000)
    ));

    let provider_advanced = crate::asr::volcengine::TargetSpeakerUpdate {
        provider_audio_duration_ms: Some(5_701),
        ..update.clone()
    };
    assert!(!super::provider_progress_stalled(
        &coordinator.inner,
        session_id,
        &provider_advanced,
        started + Duration::from_millis(1_001)
    ));
    assert!(!super::provider_progress_stalled(
        &coordinator.inner,
        new_session_id(),
        &update,
        started + Duration::from_secs(1)
    ));
}

#[test]
fn target_speaker_endpoint_terminal_wake_continuation_is_bounded_session_matched_and_one_shot() {
    let coordinator = Coordinator::new();
    let started = Instant::now();
    let session_id = new_session_id();
    let wake_pcm = vec![7u8; 32_000];

    assert!(super::stage_terminal_wake_continuation_at(
        &coordinator.inner,
        wake_pcm.clone(),
        0.8,
        "开始录音".into(),
        true,
        started
    ));
    assert!(!super::stage_terminal_wake_continuation_at(
        &coordinator.inner,
        wake_pcm.clone(),
        0.8,
        "开始录音".into(),
        true,
        started + Duration::from_millis(1)
    ));
    assert!(super::bind_terminal_wake_continuation_session_at(
        &coordinator.inner,
        session_id,
        started + Duration::from_millis(2)
    ));
    assert!(coordinator
        .inner
        .embedded_audio_automatic_wake_guard
        .lock()
        .as_ref()
        .is_some_and(|guard| guard.session_id == session_id));

    let continuation = super::take_terminal_wake_continuation_at(
        &coordinator.inner,
        session_id,
        started + Duration::from_millis(3),
    )
    .expect("matching continuation");
    assert_eq!(continuation.wake_pcm, wake_pcm);
    assert_eq!(continuation.wake_phrase, "开始录音");
    assert!(continuation.enrolled_owner_matched);
    assert!(super::take_terminal_wake_continuation_at(
        &coordinator.inner,
        session_id,
        started + Duration::from_millis(4)
    )
    .is_none());
}

#[test]
fn terminal_wake_body_release_plan_is_ordered_and_nonduplicating() {
    let body = super::TerminalWakeBody {
        candidate_id: 7,
        pcm: (0u8..12).collect(),
        candidate_range: Some(crate::observability::CandidateRange {
            start: 100,
            end: 112,
        }),
        source_runs: VecDeque::from([
            super::BufferedCandidateSourceRun {
                bytes: 4,
                capture_generation: Some(1),
                segment_id: Some(11),
                candidate_range: Some(crate::observability::CandidateRange {
                    start: 100,
                    end: 104,
                }),
                collector_metadata: None,
                collector_emitted_range: None,
                capture_admission_context: None,
            },
            super::BufferedCandidateSourceRun {
                bytes: 4,
                capture_generation: Some(1),
                segment_id: Some(12),
                candidate_range: Some(crate::observability::CandidateRange {
                    start: 104,
                    end: 108,
                }),
                collector_metadata: None,
                collector_emitted_range: None,
                capture_admission_context: None,
            },
        ]),
        source_admission_ledger: Arc::new(std::sync::Mutex::new(
            super::SourceAdmissionDependencyLedger::default(),
        )),
    };

    let pieces = super::terminal_wake_body_release_pieces(&body);
    assert_eq!(
        pieces
            .iter()
            .map(|piece| (piece.offset, piece.bytes, piece.segment_id))
            .collect::<Vec<_>>(),
        vec![(0, 4, Some(11)), (4, 4, Some(12)), (8, 4, None)]
    );
    let mut covered = Vec::new();
    for piece in pieces {
        covered.extend(piece.offset..piece.offset + piece.bytes);
    }
    assert_eq!(covered, (0..body.pcm.len()).collect::<Vec<_>>());
}

#[test]
fn terminal_wake_body_is_transferred_once_and_dropped_on_expiry_or_wrong_binding() {
    let coordinator = Coordinator::new();
    let started = Instant::now();
    let first_session_id = new_session_id();
    let body = super::TerminalWakeBody {
        candidate_id: 9,
        pcm: vec![9u8; 8],
        candidate_range: Some(crate::observability::CandidateRange { start: 20, end: 28 }),
        source_runs: VecDeque::new(),
        source_admission_ledger: Arc::new(std::sync::Mutex::new(
            super::SourceAdmissionDependencyLedger::default(),
        )),
    };
    assert!(super::stage_terminal_wake_continuation_with_body_at(
        &coordinator.inner,
        vec![1u8; 16],
        0.4,
        "开始录音".into(),
        true,
        body,
        started,
    ));
    assert!(super::bind_terminal_wake_continuation_session_at(
        &coordinator.inner,
        first_session_id,
        started + Duration::from_millis(1),
    ));
    let continuation = super::take_terminal_wake_continuation_at(
        &coordinator.inner,
        first_session_id,
        started + Duration::from_millis(2),
    )
    .expect("matching continuation");
    assert_eq!(continuation.body.pcm, vec![9u8; 8]);
    assert!(super::take_terminal_wake_continuation_at(
        &coordinator.inner,
        first_session_id,
        started + Duration::from_millis(3),
    )
    .is_none());

    let expired_body = super::TerminalWakeBody {
        candidate_id: 10,
        pcm: vec![10u8; 8],
        candidate_range: None,
        source_runs: VecDeque::new(),
        source_admission_ledger: Arc::new(std::sync::Mutex::new(
            super::SourceAdmissionDependencyLedger::default(),
        )),
    };
    let second_session_id = new_session_id();
    assert!(super::stage_terminal_wake_continuation_with_body_at(
        &coordinator.inner,
        vec![2u8; 16],
        0.4,
        "开始录音".into(),
        true,
        expired_body,
        started + Duration::from_secs(1),
    ));
    assert!(!super::bind_terminal_wake_continuation_session_at(
        &coordinator.inner,
        second_session_id,
        started + Duration::from_secs(1) + super::EMBEDDED_TERMINAL_WAKE_CONTINUATION_TTL,
    ));
    assert!(coordinator
        .inner
        .embedded_audio_terminal_wake_continuation
        .lock()
        .is_none());

    let wrong_binding_body = super::TerminalWakeBody {
        candidate_id: 11,
        pcm: vec![11u8; 8],
        candidate_range: None,
        source_runs: VecDeque::new(),
        source_admission_ledger: Arc::new(std::sync::Mutex::new(
            super::SourceAdmissionDependencyLedger::default(),
        )),
    };
    let rebound_session_id = new_session_id();
    assert!(super::stage_terminal_wake_continuation_with_body_at(
        &coordinator.inner,
        vec![3u8; 16],
        0.4,
        "开始录音".into(),
        true,
        wrong_binding_body,
        started + Duration::from_secs(2),
    ));
    assert!(super::bind_terminal_wake_continuation_session_at(
        &coordinator.inner,
        rebound_session_id,
        started + Duration::from_secs(2),
    ));
    assert!(super::take_terminal_wake_continuation_at(
        &coordinator.inner,
        new_session_id(),
        started + Duration::from_secs(2),
    )
    .is_none());
    assert!(coordinator
        .inner
        .embedded_audio_terminal_wake_continuation
        .lock()
        .is_none());
}

#[test]
fn target_speaker_endpoint_terminal_wake_continuation_expiry_or_session_mismatch_cannot_leak() {
    let coordinator = Coordinator::new();
    let started = Instant::now();
    let session_id = new_session_id();
    assert!(super::stage_terminal_wake_continuation_at(
        &coordinator.inner,
        vec![1u8; 3_200],
        0.1,
        "开始录音".into(),
        true,
        started
    ));
    assert!(!super::bind_terminal_wake_continuation_session_at(
        &coordinator.inner,
        session_id,
        started + super::EMBEDDED_TERMINAL_WAKE_CONTINUATION_TTL
    ));
    assert!(coordinator
        .inner
        .embedded_audio_terminal_wake_continuation
        .lock()
        .is_none());

    let rebound_session_id = new_session_id();
    assert!(super::stage_terminal_wake_continuation_at(
        &coordinator.inner,
        vec![2u8; 3_200],
        0.1,
        "开始录音".into(),
        true,
        started + Duration::from_secs(7)
    ));
    assert!(super::bind_terminal_wake_continuation_session_at(
        &coordinator.inner,
        rebound_session_id,
        started + Duration::from_secs(7)
    ));
    assert!(super::take_terminal_wake_continuation_at(
        &coordinator.inner,
        new_session_id(),
        started + Duration::from_secs(7)
    )
    .is_none());
    assert!(coordinator
        .inner
        .embedded_audio_terminal_wake_continuation
        .lock()
        .is_none());
    assert!(coordinator
        .inner
        .embedded_audio_automatic_wake_guard
        .lock()
        .is_none());
}

#[test]
fn target_speaker_endpoint_bound_terminal_wake_continuation_routes_next_device_segment_to_body() {
    // Installed session ca5c63d3 reproduced the regression: terminal wake
    // opened a visible Starting session, but the next VoiceActivation segment
    // was buffered as a second wake candidate and the capsule hung for 25s.
    let coordinator = Coordinator::new();
    let started = Instant::now();
    let session_id = new_session_id();
    {
        let mut state = coordinator.inner.state.lock();
        state.session_id = session_id;
        state.phase = SessionPhase::Starting;
    }
    assert!(super::stage_terminal_wake_continuation_at(
        &coordinator.inner,
        vec![3u8; 32_000],
        0.8,
        "开始录音".into(),
        true,
        started
    ));
    assert!(super::bind_terminal_wake_continuation_session_at(
        &coordinator.inner,
        session_id,
        started + Duration::from_millis(1)
    ));
    assert_eq!(
        super::terminal_wake_continuation_waiting_for_audio_at(
            &coordinator.inner,
            started + Duration::from_millis(2)
        ),
        Some(session_id)
    );

    coordinator.inner.state.lock().phase = SessionPhase::Idle;
    assert_eq!(
        super::terminal_wake_continuation_waiting_for_audio_at(
            &coordinator.inner,
            started + Duration::from_millis(3)
        ),
        None,
        "only the bound Starting session may claim the next device segment"
    );
}

#[test]
fn terminal_wake_body_route_precedes_voice_activation_candidate_classification() {
    let source = include_str!("dictation_embedded_candidate_begin.rs");
    let route = source
        .find("terminal_wake_continuation_waiting_for_audio")
        .expect("terminal continuation body route");
    let classify = source
        .find("buffered_speaker_candidate_kind")
        .expect("ordinary VoiceActivation candidate classifier");
    assert!(route < classify);
}

#[test]
fn terminal_wake_body_guard_is_bound_before_recording_capsule_emit() {
    let source = include_str!("hotkey_device_runtime.rs");
    let start = source
        .find("async fn request_embedded_ble_recording_start_from_host")
        .expect("host start function");
    let body = &source[start..];
    let bind = body
        .find("bind_terminal_wake_continuation_session")
        .expect("terminal continuation bind");
    let emit = body
        .find("emit_capsule_for_session")
        .expect("recording capsule emit");
    assert!(bind < emit);
}

#[test]
fn target_speaker_endpoint_terminal_wake_continuation_captures_original_windows_insertion_target() {
    // Installed session 237 completed ASR successfully but showed the
    // clipboard/error capsule because the host-start path explicitly created
    // the real session with `focus_target=None`.
    let source = include_str!("hotkey_device_runtime.rs");
    let start = source
        .find("async fn request_embedded_ble_recording_start_from_host")
        .expect("host start function");
    let end = source[start..]
        .find("async fn handle_device_translation_action")
        .map(|offset| start + offset)
        .expect("next function boundary");
    let body = &source[start..end];
    // r23（2026-09-18）：改成原子成对抓取 (HWND, 标题)——capture_focus_target_with_title
    // 内部同样读前台焦点目标，另带自愈所需的标题。
    assert!(body.contains("capture_focus_target_with_title()"));
    assert!(!body.contains("begin_session_state(&mut state, None"));
}

#[test]
fn target_speaker_endpoint_holds_after_one_transient_local_mismatch() {
    // Installed session 6ef0d4b8 reproduced this exact shape: cloud
    // diarization had stabilized only the wake phrase while the debounced local
    // identity still owned the continuing body through 5.2 seconds.
    let transient_mismatch = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("1".into()),
        target_speech_end_ms: Some(880),
        provider_audio_duration_ms: Some(5_500),
        audio_duration_ms: Some(5_900),
        local_speech_end_ms: Some(5_200),
        qualified_owner_speech_end_ms: Some(5_200),
        qualified_owner_activity_advanced: true,
        local_speaker_classification_kind: Some(crate::asr::volcengine::LocalSpeakerClassificationKind::Target),
        local_speaker_signal_quality_sufficient: Some(true),
        local_speaker_observation_end_ms: Some(5_200),
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: Some(5_200),
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(4_482),
        target_activity_advanced: false,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    assert!(!super::target_speaker_endpoint_due(&transient_mismatch));

    // Target clock max(880, 4482, 5200)=5200; other-speaker energy does not
    // extend it. Exact due is 5200 + 3000 = 8200 (audio clock also 3 s past the
    // trailing speech edge so the unresolved-tail hold has expired).
    let confirmed_other_speaker_tail = crate::asr::volcengine::TargetSpeakerUpdate {
        provider_audio_duration_ms: Some(8_200),
        audio_duration_ms: Some(8_600),
        local_speech_end_ms: Some(5_600),
        local_non_target_speech_end_ms: Some(5_600),
        ..transient_mismatch
    };
    assert!(super::target_speaker_endpoint_due(
        &confirmed_other_speaker_tail
    ));
}

#[test]
fn target_speaker_endpoint_holds_during_owner_identity_recovery() {
    // Installed session 53a3f755 reproduced this shape. The provider had
    // already heard the continuing tail while the first recovering Target
    // window was still waiting for its second hysteresis confirmation.
    let first_recovering_target = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("0".into()),
        target_speech_end_ms: Some(3_152),
        provider_audio_duration_ms: Some(6_300),
        audio_duration_ms: Some(4_500),
        local_speech_end_ms: Some(4_500),
        qualified_owner_speech_end_ms: Some(3_300),
        qualified_owner_activity_advanced: true,
        local_speaker_classification_kind: Some(crate::asr::volcengine::LocalSpeakerClassificationKind::Target),
        local_speaker_signal_quality_sufficient: Some(true),
        local_speaker_observation_end_ms: Some(3_300),
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: Some(3_300),
        local_non_target_speech_end_ms: Some(4_100),
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(3_152),
        target_activity_advanced: false,
        pending_unattributed_speech: true,
        pending_activity_advanced: true,
        speaker_info_present: true,
    };

    // The only continuing local evidence is a generic/non-target edge. It
    // must not keep the owner's recording open while the provider row is
    // pending.
    assert!(super::target_speaker_endpoint_due(&first_recovering_target));

    let owner_restored = crate::asr::volcengine::TargetSpeakerUpdate {
        provider_audio_duration_ms: Some(4_600),
        audio_duration_ms: Some(4_900),
        local_speech_end_ms: Some(4_900),
        local_target_speech_end_ms: Some(4_900),
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        ..first_recovering_target
    };
    assert!(!super::target_speaker_endpoint_due(&owner_restored));
}

#[test]
fn target_speaker_endpoint_waits_for_startup_body_calibration() {
    let unresolved_body = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("0".into()),
        target_speech_end_ms: Some(1_442),
        provider_audio_duration_ms: Some(4_442),
        audio_duration_ms: Some(4_600),
        local_speech_end_ms: Some(3_100),
        qualified_owner_speech_end_ms: None,
        qualified_owner_activity_advanced: false,
        local_speaker_classification_kind: None,
        local_speaker_signal_quality_sufficient: None,
        local_speaker_observation_end_ms: None,
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: None,
        local_non_target_speech_end_ms: Some(3_100),
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(1_442),
        target_activity_advanced: false,
        pending_unattributed_speech: true,
        pending_activity_advanced: true,
        speaker_info_present: true,
    };
    // Pending body speech without a positive local owner watermark is not an
    // owner tail and cannot block endpointing.
    assert!(super::target_speaker_endpoint_due(&unresolved_body));

    // Once the provider has converged and there is still only confirmed
    // non-target body speech, the wake speaker's exact endpoint remains bounded.
    let confirmed_other = crate::asr::volcengine::TargetSpeakerUpdate {
        provider_audio_duration_ms: Some(4_442),
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        ..unresolved_body
    };
    assert!(super::target_speaker_endpoint_due(&confirmed_other));
}

#[test]
fn incomplete_and_short_body_previews_still_follow_owner_activity() {
    assert_eq!(
        super::target_speaker_end_timeout_ms_for_preview(Some("你帮")),
        super::EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS
    );
    assert_eq!(
        super::target_speaker_end_timeout_ms_for_preview(Some("那你")),
        super::EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS
    );
    assert_eq!(
        super::target_speaker_end_timeout_ms_for_preview(Some("你继续帮我看一下吧")),
        super::EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS
    );
    assert_eq!(
        super::target_speaker_end_timeout_ms_for_preview(Some("你帮。")),
        super::EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS
    );
    assert_eq!(
        super::target_speaker_end_timeout_ms_for_preview(Some("我先检查，然后")),
        super::EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS
    );
    // Empty / no body uses the same base window; no-body abandon (8 s) is
    // layered separately in resolve_target_speaker_endpoint_policy.
    assert_eq!(
        super::target_speaker_end_timeout_ms_for_preview(None),
        super::EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS
    );
    assert_eq!(
        super::target_speaker_end_timeout_ms_for_preview(Some("   ")),
        super::EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS
    );
}

#[test]
fn host_started_wake_guard_survives_embedded_session_begin() {
    // terminal_wake_body_continuation / host start arms the wake guard before
    // BLE PCM attaches. begin_embedded_audio_dictation_session_id reuses the
    // Starting session; the guard for that session must not be wiped or the
    // no-body path falls back to snappy 1.0s.
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    {
        let mut state = coordinator.inner.state.lock();
        state.session_id = session_id;
        state.phase = SessionPhase::Starting;
    }
    arm_automatic_wake_text_guard(&coordinator.inner, session_id, "开始录音".into(), 0);
    assert!(automatic_wake_session_active(
        &coordinator.inner,
        session_id
    ));
    assert!(!automatic_wake_body_started(&coordinator.inner, session_id));

    // Mirror begin_embedded_audio_dictation_session preserve rule.
    if !automatic_wake_session_active(&coordinator.inner, session_id) {
        clear_automatic_wake_text_guard(&coordinator.inner);
    }
    assert!(
        automatic_wake_session_active(&coordinator.inner, session_id),
        "host-started wake guard must survive embedded session begin attach"
    );

    let mode_timeout = super::target_speaker_end_timeout_ms_for_preview(None);
    let no_body_timeout = if automatic_wake_body_started(&coordinator.inner, session_id) {
        mode_timeout
    } else if automatic_wake_session_active(&coordinator.inner, session_id) {
        super::EMBEDDED_AUTOMATIC_WAKE_NO_BODY_END_TIMEOUT_MS.max(mode_timeout)
    } else {
        mode_timeout
    };
    assert_eq!(no_body_timeout, 8_000);
    assert_eq!(
        super::target_speaker_inactive_stop_reason(no_body_timeout),
        "target_speaker_inactive_no_body_8000ms"
    );
}

#[test]
fn automatic_wake_no_body_uses_longer_endpoint_timeout() {
    // Session 72519330: after visible capsule, the 1.0s snappy
    // endpoint still measured the wake-phrase clock and empty-ended. Wake
    // sessions without body text must use the 3.0s abandon timeout; once body
    // starts, standard 1.0s returns.
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    arm_automatic_wake_text_guard(&coordinator.inner, session_id, "开始录音".into(), 1_200);
    acknowledge_automatic_wake_capsule_visible(&coordinator.inner, session_id);

    assert!(automatic_wake_session_active(
        &coordinator.inner,
        session_id
    ));
    assert!(!automatic_wake_body_started(&coordinator.inner, session_id));

    let mode_timeout = super::target_speaker_end_timeout_ms_for_preview(None);
    assert_eq!(mode_timeout, 3_000);
    let no_body_timeout = if automatic_wake_body_started(&coordinator.inner, session_id) {
        mode_timeout
    } else if automatic_wake_session_active(&coordinator.inner, session_id) {
        super::EMBEDDED_AUTOMATIC_WAKE_NO_BODY_END_TIMEOUT_MS.max(mode_timeout)
    } else {
        mode_timeout
    };
    assert_eq!(no_body_timeout, 8_000);
    assert_eq!(
        super::target_speaker_inactive_stop_reason(no_body_timeout),
        "target_speaker_inactive_no_body_8000ms"
    );

    // Wake-only target clock at 1332ms is not yet due at +1000 under no-body
    // timeout (would have been due under snappy 1.0s).
    let wake_only = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("0".into()),
        target_speech_end_ms: Some(1_332),
        provider_audio_duration_ms: Some(2_500),
        audio_duration_ms: Some(2_500),
        local_speech_end_ms: Some(2_500),
        qualified_owner_speech_end_ms: None,
        qualified_owner_activity_advanced: false,
        local_speaker_classification_kind: None,
        local_speaker_signal_quality_sufficient: None,
        local_speaker_observation_end_ms: None,
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: None,
        local_non_target_speech_end_ms: Some(2_500),
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(1_332),
        target_activity_advanced: false,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    assert!(super::target_speaker_endpoint_due_with_timeout(
        &wake_only, 1_000
    ));
    assert!(!super::target_speaker_endpoint_due_with_timeout(
        &wake_only, 3_000
    ));
    let abandoned = crate::asr::volcengine::TargetSpeakerUpdate {
        provider_audio_duration_ms: Some(4_400),
        audio_duration_ms: Some(4_400),
        ..wake_only
    };
    assert!(super::target_speaker_endpoint_due_with_timeout(
        &abandoned, 3_000
    ));

    // Body text latch restores snappy 1.0s.
    assert_eq!(
        filter_automatic_wake_text(
            &coordinator.inner,
            session_id,
            "开始录音。今天继续测试。",
            true,
        ),
        "今天继续测试。"
    );
    assert!(automatic_wake_body_started(&coordinator.inner, session_id));
    assert_eq!(
        super::target_speaker_inactive_stop_reason(1_000),
        "target_speaker_inactive_3000ms"
    );
}

#[test]
fn automatic_wake_phrase_only_preview_does_not_latch_body_or_snappy_endpoint() {
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    arm_automatic_wake_text_guard(&coordinator.inner, session_id, "开始录音".into(), 1_200);
    acknowledge_automatic_wake_capsule_visible(&coordinator.inner, session_id);

    assert_eq!(
        filter_automatic_wake_text(&coordinator.inner, session_id, "开始录音", true),
        ""
    );
    assert!(!automatic_wake_body_started(&coordinator.inner, session_id));
    assert_eq!(
        filter_automatic_wake_text(&coordinator.inner, session_id, "开始录音。", true),
        ""
    );
    assert!(!automatic_wake_body_started(&coordinator.inner, session_id));
    assert_eq!(
        filter_automatic_wake_text(&coordinator.inner, session_id, "嗯，开始录音", true),
        ""
    );
    assert!(!automatic_wake_body_started(&coordinator.inner, session_id));

    let policy = super::resolve_target_speaker_endpoint_policy(
        &coordinator.inner,
        session_id,
        Some("开始录音"),
        Some(2_500),
    );
    assert!(!policy.body_started);
    assert_eq!(policy.endpoint_timeout_ms, 8_000);
    assert_eq!(policy.stop_reason, "target_speaker_inactive_no_body_8000ms");

    assert_eq!(
        filter_automatic_wake_text(
            &coordinator.inner,
            session_id,
            "开始录音，今天继续测试",
            true,
        ),
        "今天继续测试"
    );
    assert!(automatic_wake_body_started(&coordinator.inner, session_id));
    let body_policy = super::resolve_target_speaker_endpoint_policy(
        &coordinator.inner,
        session_id,
        Some("今天继续测试"),
        Some(3_200),
    );
    assert!(body_policy.body_started);
    assert_eq!(body_policy.endpoint_timeout_ms, 3_000);
}

#[test]
fn automatic_wake_target_speaker_endpoint_uses_one_policy_snapshot_for_decision_and_reason() {
    // Live session 1896 exposed a split policy: callback/watchdog committed on
    // the 900 ms body wall clock, while stop dispatch recomputed and logged the
    // 3000 ms no-body reason. The resolved snapshot is now the only value both
    // layers may consume.
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    arm_automatic_wake_text_guard(&coordinator.inner, session_id, "开始录音".into(), 1_200);
    acknowledge_automatic_wake_capsule_visible(&coordinator.inner, session_id);

    let policy = super::resolve_target_speaker_endpoint_policy(
        &coordinator.inner,
        session_id,
        None,
        Some(2_500),
    );
    assert!(!policy.body_started);
    assert!(policy.initial_body_wait_active);
    assert!(policy.automatic_no_body_started_at.is_some());
    assert_eq!(policy.endpoint_timeout_ms, 8_000);
    assert_eq!(policy.wall_clock_timeout_ms, 7_900);
    assert_eq!(policy.stop_reason, "target_speaker_inactive_no_body_8000ms");

    let callback_source = include_str!("dictation_volcengine_callbacks.rs");
    let watchdog_source = include_str!("dictation_endpoint_clock.rs");
    let stop_source = include_str!("dictation_target_speaker_update.rs");
    assert!(callback_source.contains("resolve_target_speaker_endpoint_policy"));
    assert!(watchdog_source.contains("resolve_target_speaker_endpoint_policy"));
    assert!(callback_source.contains("reduce_session_policy"));
    assert!(watchdog_source.contains("reduce_session_policy"));
    assert!(!callback_source.contains("target_speaker_end_timeout_ms_for_preview"));
    assert!(!watchdog_source.contains("target_speaker_end_timeout_ms_for_preview("));
    assert!(!stop_source.contains("target_speaker_end_timeout_ms_for_preview"));
    assert!(!stop_source.contains("endpoint_decision_committed"));
}

#[test]
fn automatic_endpoint_does_not_stop_during_a_new_owner_compatible_vad_onset() {
    // Installed session 651ad979: PendingSpeech began 258 ms before the
    // three-second owner deadline, but automatic STOP ignored the VAD and
    // preceded canonical Speech by 90 ms. This is a STOP veto, not a new
    // owner watermark; an explicit recent NonTarget must still be stoppable.
    use crate::asr::volcengine::{
        LocalSpeechActivityState, LocalSpeechEvidence, TargetSpeakerUpdate,
    };
    let started = std::time::Instant::now();
    let update = TargetSpeakerUpdate {
        speaker_id: Some("0".into()),
        target_speech_end_ms: Some(23_672),
        provider_audio_duration_ms: Some(24_400),
        audio_duration_ms: Some(27_500),
        local_speech_end_ms: Some(27_500),
        qualified_owner_speech_end_ms: Some(24_200),
        qualified_owner_activity_advanced: false,
        local_speaker_classification_kind: None,
        local_speaker_signal_quality_sufficient: None,
        local_speaker_observation_end_ms: Some(27_400),
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: Some(24_200),
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(23_672),
        target_activity_advanced: false,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    let pending = LocalSpeechEvidence {
        analyzed_through_ms: 27_400,
        pending_speech_start_ms: Some(27_264),
        activity_epoch: 4,
        revision: 10,
        state: LocalSpeechActivityState::PendingSpeech,
        ..Default::default()
    };
    assert!(super::automatic_vad_candidate_holds_stop(pending, &update, true));
    assert!(!super::automatic_vad_candidate_holds_stop(pending, &update, false));

    let mut clock = super::SettledTargetEndpointClock::default();
    clock.automatic_wake_session = true;
    clock.armed_at = Some(started);
    clock.armed_target_end_ms = Some(24_200);
    clock.last_positive_owner_evidence_at = Some(started);
    clock.latest_update = Some(update.clone());
    clock.latest_update_at = Some(started + std::time::Duration::from_millis(2_900));
    clock.product_endpoint.arm(super::product_endpoint_evidence(
        &update,
        clock.armed_target_end_ms,
        true,
    ));
    clock.note_local_vad_evidence(pending);
    assert!(clock
        .latest_due_update(started + std::time::Duration::from_millis(2_950), 2_900)
        .is_none());
    assert_eq!(
        clock.take_due_hold_diagnostic(),
        Some((0, "automatic_vad_speech_pending")),
    );
    assert!(!clock.positive_owner_evidence_live(
        started + std::time::Duration::from_millis(4_000)
    ));

    let foreign = TargetSpeakerUpdate {
        local_non_target_speech_end_ms: Some(27_450),
        ..update.clone()
    };
    assert!(!super::automatic_vad_candidate_holds_stop(pending, &foreign, true));
    let quiet = LocalSpeechEvidence {
        state: LocalSpeechActivityState::NonSpeech,
        ..pending
    };
    assert!(!super::automatic_vad_candidate_holds_stop(quiet, &update, true));
    let lagging = LocalSpeechEvidence {
        analyzed_through_ms: 27_000,
        ..pending
    };
    assert!(!super::automatic_vad_candidate_holds_stop(lagging, &update, true));
}

#[test]
fn automatic_wake_target_speaker_endpoint_no_body_uses_original_guard_clock() {
    // Live session 2898 accepted the owner wake and showed Recording, then
    // the pre-activation BLE segment rotated before any body arrived. With no
    // provider/preview callback, the old endpoint clock was never armed and
    // the session survived until the provider's eight-second transport error.
    let started = std::time::Instant::now();
    let wake_only_snapshot = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: None,
        target_speech_end_ms: None,
        provider_audio_duration_ms: None,
        audio_duration_ms: Some(1_497),
        local_speech_end_ms: Some(1_497),
        qualified_owner_speech_end_ms: Some(1_497),
        qualified_owner_activity_advanced: true,
        local_speaker_classification_kind: Some(crate::asr::volcengine::LocalSpeakerClassificationKind::Target),
        local_speaker_signal_quality_sufficient: Some(true),
        local_speaker_observation_end_ms: Some(1_497),
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: Some(1_497),
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: None,
        target_activity_advanced: false,
        pending_unattributed_speech: true,
        pending_activity_advanced: false,
        speaker_info_present: false,
    };
    let waiting = super::TargetSpeakerEndpointPolicy {
        body_started: false,
        initial_body_wait_active: true,
        automatic_no_body_started_at: Some(started),
        endpoint_timeout_ms: 3_000,
        wall_clock_timeout_ms: 2_900,
        stop_reason: "target_speaker_inactive_no_body_8000ms",
    };
    let mut clock = super::SettledTargetEndpointClock::default();

    assert!(clock
        .reduce_session_policy(
            started + std::time::Duration::from_millis(500),
            waiting,
            &wake_only_snapshot,
            true,
        )
        .is_none());
    assert_eq!(
        clock.lifecycle(),
        crate::speech_decision_kernel::OwnerEndpointState::QuietPending,
        "the accepted wake-only session must enter the sole endpoint controller"
    );
    assert!(clock.automatic_no_body_armed);
    assert_eq!(
        clock.arm_latest_for_visible_body(
            started + std::time::Duration::from_millis(700),
            false,
        ),
        None,
        "wake-only preview callbacks must not release the no-body endpoint mode"
    );
    assert!(clock.automatic_no_body_armed);

    let expired = super::TargetSpeakerEndpointPolicy {
        initial_body_wait_active: false,
        ..waiting
    };
    assert!(
        clock
            .reduce_session_policy(
                started + std::time::Duration::from_millis(3_000),
                expired,
                &wake_only_snapshot,
                true,
            )
            .is_some(),
        "wake audio, provider pending and an obsolete classifier must not create a second wait"
    );
    assert_eq!(
        clock.lifecycle(),
        crate::speech_decision_kernel::OwnerEndpointState::QuietPending
    );
}

#[test]
fn automatic_wake_waits_for_new_body_audio_before_cloud_first_text() {
    let started = std::time::Instant::now();
    let initial = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: None,
        target_speech_end_ms: None,
        provider_audio_duration_ms: None,
        audio_duration_ms: Some(1_500),
        local_speech_end_ms: Some(1_500),
        qualified_owner_speech_end_ms: Some(1_500),
        qualified_owner_activity_advanced: true,
        local_speaker_classification_kind: Some(crate::asr::volcengine::LocalSpeakerClassificationKind::Target),
        local_speaker_signal_quality_sufficient: Some(true),
        local_speaker_observation_end_ms: Some(1_500),
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: Some(1_500),
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: None,
        target_activity_advanced: false,
        pending_unattributed_speech: true,
        pending_activity_advanced: false,
        speaker_info_present: false,
    };
    let policy = super::TargetSpeakerEndpointPolicy {
        body_started: false,
        initial_body_wait_active: false,
        automatic_no_body_started_at: Some(started),
        endpoint_timeout_ms: 3_000,
        wall_clock_timeout_ms: 2_900,
        stop_reason: "target_speaker_inactive_no_body_8000ms",
    };
    let mut clock = super::SettledTargetEndpointClock::default();
    assert!(clock
        .reduce_session_policy(started, policy, &initial, false)
        .is_none());
    let mut body = initial;
    // The historical clip's cloud first result takes over seven seconds.
    // Continuous post-wake speech must survive the three-second no-text mark.
    for seconds in 1..=8 {
        body.audio_duration_ms = Some(1_500 + seconds * 1_000);
        body.local_speech_end_ms = Some(1_400 + seconds * 1_000);
        assert!(clock
            .reduce_session_policy(
                started + std::time::Duration::from_secs(seconds),
                policy,
                &body,
                false
            )
            .is_none());
    }
    assert!(clock
        .reduce_session_policy(
            started + std::time::Duration::from_millis(10_800),
            policy,
            &body,
            false
        )
        .is_none());
    assert!(
        clock
            .reduce_session_policy(
                started + std::time::Duration::from_millis(11_100),
                policy,
                &body,
                false
            )
            .is_some(),
        "repeated old speech must still end after bounded silence"
    );
}

#[test]
fn automatic_wake_target_speaker_endpoint_body_replaces_no_body_deadline() {
    let started = std::time::Instant::now();
    let wake_only_snapshot = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: None,
        target_speech_end_ms: None,
        provider_audio_duration_ms: None,
        audio_duration_ms: Some(1_500),
        local_speech_end_ms: Some(1_500),
        qualified_owner_speech_end_ms: Some(1_500),
        qualified_owner_activity_advanced: true,
        local_speaker_classification_kind: Some(crate::asr::volcengine::LocalSpeakerClassificationKind::Target),
        local_speaker_signal_quality_sufficient: Some(true),
        local_speaker_observation_end_ms: Some(1_500),
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: Some(1_500),
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: None,
        target_activity_advanced: false,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: false,
    };
    let waiting = super::TargetSpeakerEndpointPolicy {
        body_started: false,
        initial_body_wait_active: true,
        automatic_no_body_started_at: Some(started),
        endpoint_timeout_ms: 3_000,
        wall_clock_timeout_ms: 2_900,
        stop_reason: "target_speaker_inactive_no_body_8000ms",
    };
    let mut clock = super::SettledTargetEndpointClock::default();
    assert!(clock
        .reduce_session_policy(started, waiting, &wake_only_snapshot, false)
        .is_none());

    let body_at = started + std::time::Duration::from_millis(2_500);
    let body = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("0".into()),
        target_speech_end_ms: Some(3_900),
        provider_audio_duration_ms: Some(4_000),
        audio_duration_ms: Some(4_000),
        local_speech_end_ms: Some(3_900),
        local_target_speech_end_ms: Some(3_900),
        target_activity_advanced: true,
        speaker_info_present: true,
        ..wake_only_snapshot
    };
    let generation = clock
        .observe(&body, true, body_at)
        .expect("first body must replace the wake-only candidate with the owner clock");
    assert!(
        clock
            .due_update(
                generation,
                started + std::time::Duration::from_millis(3_000),
                900,
            )
            .is_none(),
        "the obsolete no-body deadline must not cut off newly accepted body speech"
    );
    assert!(clock
        .due_update(
            generation,
            body_at + std::time::Duration::from_millis(900),
            900,
        )
        .is_some());
}

#[test]
fn target_speaker_endpoint_rotated_segment_handoff_is_single_use() {
    let coordinator = Coordinator::new();
    let session_id = new_session_id();

    super::mark_embedded_ble_awaiting_post_activation_segment(&coordinator.inner, session_id);
    assert!(super::take_embedded_ble_awaiting_post_activation_segment(
        &coordinator.inner,
        session_id,
    ));
    assert!(
        !super::take_embedded_ble_awaiting_post_activation_segment(&coordinator.inner, session_id,),
        "one logical endpoint may own the rotated-segment finalization only once"
    );

    super::mark_embedded_ble_awaiting_post_activation_segment(&coordinator.inner, session_id);
    assert!(super::clear_embedded_ble_awaiting_post_activation_segment(
        &coordinator.inner,
        session_id,
    ));
    assert!(
        !super::take_embedded_ble_awaiting_post_activation_segment(&coordinator.inner, session_id,),
        "a real post-activation segment must cancel external no-body finalization"
    );
}

#[test]
fn target_speaker_endpoint_no_body_finalization_cannot_steal_next_physical_wake() {
    // Session 2899 reached the no-body endpoint, but the old actor waited for
    // a hypothetical post-activation segment. Forty-two seconds later it
    // attached segment 2900 to the dead session. Keep the physical/logical
    // hand-off explicit and prove every owner of it is present in source.
    let stream = include_str!("dictation_embedded_stream.rs");
    let begin = include_str!("dictation_embedded_candidate_begin.rs");
    let endpoint = include_str!("dictation_target_speaker_update.rs");
    let loop_source = include_str!("dictation_embedded_submit.rs");

    assert!(stream.contains("mark_embedded_ble_awaiting_post_activation_segment"));
    assert!(stream.contains("bind_post_activation_segment"));
    assert!(begin.contains("clear_embedded_ble_awaiting_post_activation_segment"));
    assert!(endpoint.contains("take_embedded_ble_awaiting_post_activation_segment"));
    assert!(endpoint.contains("logical_no_body_after_rotated_segment"));
    assert!(loop_source.contains("release_externally_finalized_session_if_needed"));
}

#[test]
fn target_speaker_endpoint_reduces_fresh_activity_before_stop_policy() {
    // A fresh provider/local identity callback is an evidence event even when
    // no stop is due. It may renew the physical firmware lease, but it must not
    // mirror endpoint sub-states into RecordingLifecycleController.
    let callback_source = include_str!("dictation_volcengine_callbacks.rs");
    let activity = callback_source
        .find("renew_firmware_lease_from_owner_observation(")
        .expect("every target-speaker callback must reduce fresh owner evidence");
    let decision = callback_source[activity..]
        .find("reduce_session_policy")
        .map(|offset| activity + offset)
        .expect("the same callback must then ask the endpoint reducer for a stop decision");
    assert!(
        activity < decision,
        "owner activity must renew the physical lease before endpoint stop evaluation"
    );
    assert!(
        !callback_source.contains(".recording_lifecycle"),
        "evidence callbacks must not maintain a second copy of endpoint lifecycle state"
    );

    let stop_source = include_str!("dictation_target_speaker_update.rs");
    let stop_body = stop_source
        .split("fn handle_target_speaker_endpoint_stop(")
        .nth(1)
        .expect("dedicated endpoint stop handler");
    assert!(
        !stop_body.contains("note_owner_activity(session_id)"),
        "the irreversible stop handler must not double as a fresh evidence reducer"
    );
}

#[test]
fn manual_endpoint_uses_live_pcm_when_provider_stops_publishing_speaker_callbacks() {
    let now = Instant::now();
    let mut update = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: None,
        target_speech_end_ms: None,
        provider_audio_duration_ms: Some(6_100),
        audio_duration_ms: Some(6_200),
        local_speech_end_ms: Some(6_200),
        qualified_owner_speech_end_ms: None,
        qualified_owner_activity_advanced: false,
        local_speaker_classification_kind: None,
        local_speaker_signal_quality_sufficient: None,
        local_speaker_observation_end_ms: None,
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: None,
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: false,
        stable_attributed_speech_end_ms: None,
        target_activity_advanced: false,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: false,
    };
    let policy = super::TargetSpeakerEndpointPolicy {
        body_started: true,
        initial_body_wait_active: false,
        automatic_no_body_started_at: None,
        endpoint_timeout_ms: 2_500,
        wall_clock_timeout_ms: 2_400,
        stop_reason: "target_speaker_inactive_2500ms",
    };
    let mut clock = super::SettledTargetEndpointClock::default();
    clock.observe(&update, true, now);
    clock.note_visible_body_boundary(false, 11, now);
    // A quiet provider is not a quiet microphone. Several seconds of ongoing
    // speech must survive even without a speaker id or another preview event.
    for elapsed in (100..=5_000).step_by(100) {
        update.audio_duration_ms = Some(6_200 + elapsed);
        update.local_speech_end_ms = update.audio_duration_ms;
        assert!(clock
            .reduce_session_policy(now + Duration::from_millis(elapsed), policy, &update, false,)
            .is_none());
    }
    // Real trailing silence still finishes; a repeated frozen snapshot must
    // not continually refresh the evidence age.
    update.audio_duration_ms = Some(14_400);
    assert!(clock
        .reduce_session_policy(now + Duration::from_millis(8_200), policy, &update, false,)
        .is_some());
}

#[test]
fn target_speaker_endpoint_has_one_identity_scoped_stop_commit() {
    let kernel = include_str!("../speech_decision_kernel.rs");
    let endpoint = include_str!("dictation_target_speaker_update.rs");
    let stream = include_str!("dictation_embedded_stream.rs");
    let completion = include_str!("dictation_embedded_stream_completion.rs");
    let submit = include_str!("dictation_embedded_submit.rs");
    let wake = include_str!("dictation_wake_polish.rs");
    let combined = [stream, completion, submit].join("\n");
    let coordinator_state_writers = [
        include_str!("dictation.rs"),
        include_str!("dictation_device_ai.rs"),
        include_str!("support.rs"),
    ]
    .join("\n");

    assert!(!kernel.contains("StopCommitted"));
    assert!(!kernel.contains("fn close(&mut self, coordinator_session_id: Option"));
    assert!(!combined.contains("close(None)"));
    assert!(!combined.contains("reset_product_lifecycle"));
    assert!(!wake.contains("WakeCandidateController"));
    assert!(!wake.contains("HIDDEN_AUTOMATIC_CANDIDATE_"));
    assert!(!endpoint.contains("RecordingLifecycleState::Idle"));
    assert!(!coordinator_state_writers.contains("state.phase = SessionPhase"));
    assert!(!coordinator_state_writers.contains("cleanup_cancelled_processing_session"));

    let dispatch_latch = endpoint
        .find(".compare_exchange(false, true")
        .expect("physical stop dispatch latch");

    let transport_stop = endpoint
        .find("request_embedded_ble_recording_stop_from_host_for_endpoint(")
        .expect("ticketed physical stop write");
    assert!(dispatch_latch < transport_stop);
    let public_stop_feedback = endpoint
        .find("request_embedded_audio_stop_feedback(&inner, stop_reason)")
        .expect("public transcribing transition");
    assert!(transport_stop < public_stop_feedback);
    assert!(!endpoint.contains("asr.send_last_frame().await"),
        "STOP write acknowledgement is not PCM drain completion; the device still has captured audio to deliver");
    let completion_flush = stream
        .find("session.flush_streaming_pcm();")
        .expect("flush trailing provider block");
    let completion_final = stream[completion_flush..]
        .find("end_embedded_ble_session_with_source_integrity(")
        .expect("physical completion finalizes only after the trailing PCM flush");
    assert!(completion_final > 0);

    let transport = include_str!("dictation.rs");
    let candidate_stop_dispatcher = transport
        .split("fn dispatch_owned_candidate_transport_stop")
        .nth(1)
        .expect("candidate transport stop dispatcher");
    let ownership_check = candidate_stop_dispatcher
        .find("rejected_candidate_still_owns_transport_stop(candidate_id)")
        .expect("candidate stop ownership check");
    let dispatcher_write = candidate_stop_dispatcher
        .find("send_recording_control_stop(")
        .expect("candidate stop transport write");
    assert!(ownership_check < dispatcher_write);
    assert!(
        !transport
            .split("fn reject_hidden_automatic_candidate")
            .nth(1)
            .unwrap_or_default()
            .contains("send_recording_control_stop("),
        "hidden candidate rejection must not bypass the shared stop dispatcher"
    );
    let stop_transport = transport
        .split("pub(super) async fn request_embedded_ble_recording_stop_from_host")
        .nth(1)
        .expect("stop transport function")
        .split("fn activate_embedded_audio_dictation_session")
        .next()
        .expect("bounded stop transport function");
    assert!(!stop_transport.contains("request_embedded_audio_stop_feedback"));
    let lifecycle_commit = stop_transport
        .find("commit_recording_stop(inner, session_id, reason)")
        .expect("identity-scoped lifecycle stop commit");
    let physical_stop = stop_transport
        .find("send_recording_control_stop(")
        .expect("physical stop transport");
    let failed_stop_reopen = stop_transport
        .find("reopen_recording_stop(inner, session_id)")
        .expect("failed physical stop rollback");
    assert!(lifecycle_commit < physical_stop);
    assert!(physical_stop < failed_stop_reopen);
    assert!(!include_str!("hotkey_device_runtime.rs").contains("send_recording_control_stop"));
    assert!(!include_str!("dictation_embedded_stream.rs").contains("send_recording_control_stop"));
}

#[test]
fn target_speaker_endpoint_stop_ticket_is_revocable_and_one_shot() {
    let session_id = new_session_id();
    let mut dispatch = super::EndpointStopDispatchState::default();
    let first = dispatch
        .propose(session_id, 7, 11, 3, true)
        .expect("first endpoint proposal");
    assert!(dispatch.is_current(first));
    assert!(dispatch.cancel_if_current(first));
    assert!(!dispatch.is_current(first));

    let second = dispatch
        .propose(session_id, 8, 12, 4, true)
        .expect("watchdog stays retryable after a revoked proposal");
    assert_ne!(first.proposal_id, second.proposal_id);
    assert!(!dispatch.cancel_if_current(first));
    assert!(dispatch.is_current(second));
    assert!(dispatch.begin_sending(second));
    assert!(dispatch.mark_sent_if_current(second));
    assert!(!dispatch.cancel_if_current(second));
}

#[test]
fn target_speaker_endpoint_stop_ticket_rejects_stale_generation_before_send() {
    let session_id = new_session_id();
    let mut dispatch = super::EndpointStopDispatchState::default();
    let old = dispatch
        .propose(session_id, 10, 21, 5, false)
        .expect("old endpoint proposal");
    assert!(dispatch.cancel_if_current(old));
    let current = dispatch
        .propose(session_id, 11, 22, 6, false)
        .expect("new generation proposal");
    assert_ne!(old.endpoint_generation, current.endpoint_generation);
    assert!(!dispatch.begin_sending(old));
    assert!(dispatch.begin_sending(current));
}

#[test]
fn target_speaker_endpoint_preview_has_one_session_scoped_reducer() {
    let coordinator = include_str!("../coordinator.rs");
    let preview = include_str!("dictation_preview.rs");
    let kernel = include_str!("../speech_decision_kernel.rs");

    assert!(coordinator.contains("embedded_audio_preview:"));
    assert!(!coordinator.contains("embedded_audio_partial_preview: Mutex"));
    assert!(!coordinator.contains("embedded_audio_visual_preview: Mutex"));
    assert!(kernel.contains("struct RecordingPreviewController"));
    assert_eq!(preview.matches(".observe_authoritative(").count(), 1);
    assert_eq!(preview.matches(".observe_provisional(").count(), 1);
    assert!(!preview.contains("fn provider_preview_change("));
    assert!(!preview.contains("fn stabilize_embedded_audio_partial_preview("));
    assert!(!preview.contains("fn stabilize_embedded_audio_final_supplemental_preview("));
}

#[test]
fn target_speaker_endpoint_binds_lifecycle_before_every_visible_activation() {
    let stream = include_str!("dictation_embedded_stream.rs");
    let session = include_str!("dictation_embedded_stream_session.rs");
    let promotions = stream
        .match_indices(".promote_candidate_to_owner(")
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let activations = stream
        .match_indices("if !activate_embedded_audio_dictation_session(")
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    assert_eq!(promotions.len(), 3);
    assert_eq!(activations.len(), 3);
    for (promote, activate) in promotions.into_iter().zip(activations) {
        assert!(promote < activate);
    }

    let manual_bind = session
        .find(".begin_manual_owner(")
        .expect("manual lifecycle bind");
    let continuation_branch = session
        .find("if terminal_wake_continuation.is_some()")
        .expect("terminal continuation lifecycle branch");
    let continuation_promote = session
        .find("lifecycle.promote_candidate_to_owner(candidate_id, session.session_id)")
        .expect("terminal continuation candidate promotion");
    let manual_activate = session
        .find("if !activate_embedded_audio_dictation_session(")
        .expect("manual visible activation");
    assert!(continuation_branch < continuation_promote);
    assert!(continuation_promote < manual_activate);
    assert!(manual_bind < manual_activate);
    assert_eq!(session.matches(".begin_manual_owner(").count(), 1);

    let coordinator = include_str!("dictation.rs");
    assert!(coordinator.contains("schedule_terminal_wake_continuation_expiry"));
    assert!(coordinator.contains("close_candidate(candidate_id)"));
}

#[test]
fn target_speaker_endpoint_has_one_atomic_final_transcript_arbiter() {
    let kernel = include_str!("../speech_decision_kernel.rs");
    assert!(
        !kernel.contains("EndpointArbiter"),
        "the removed endpoint compatibility type must not return"
    );
    let provider = include_str!("../asr/volcengine.rs");
    assert_eq!(
        provider
            .matches("arbitrate_final_transcript(evidence)")
            .count(),
        1,
        "the provider adapter must ask exactly one final transcript authority"
    );
    for legacy_selector in [
        "prefer_final_unfiltered_provider_text",
        "prefer_final_provider_text",
        "prefer_final_optimistic",
        "confirmed_owner_final_preview_fallback",
        "protocol final weaker than session ledger",
    ] {
        assert!(
            !provider.contains(legacy_selector),
            "legacy independent final selector returned: {legacy_selector}"
        );
    }
    assert!(provider.contains("let state = self.state.lock();"));
    assert!(provider.contains("FinalTranscriptEvidence"));
    assert!(provider.contains("explicit_non_owner_tail"));
    assert_eq!(
        provider
            .matches("final arbitration sealed authority=")
            .count(),
        1,
        "a protocol final must be sealed exactly once before terminal delivery"
    );
    assert!(
        provider.contains("FinalTranscriptAuthority::SessionLedgerRecovery"),
        "the anti-truncation ledger must be a candidate of the sole arbiter, not a later writer"
    );
    assert!(
        provider.contains("commit_once(&full_text, false)"),
        "terminal delivery must not run a second raw-provider fallback after sealing"
    );
    assert!(!provider.contains("post_stop_preview_ceiling"));
    let final_candidate_block = provider
        .split("let mut candidate = if matches!(")
        .nth(1)
        .expect("provider must assemble one terminal candidate");
    let safety_ceiling = final_candidate_block
        .find("owner_preview_safety_ceiling(")
        .expect("stop-boundary safety ceiling must normalize the candidate");
    let content_seal = final_candidate_block
        .find("let arbitrated_final_content_len")
        .expect("normalized candidate must then be sealed");
    assert!(
        safety_ceiling < content_seal,
        "the shrink-only preview ceiling must run before the provider seal"
    );

    let coordinator = include_str!("dictation.rs");
    assert_eq!(
        coordinator
            .matches("arbitrate_product_final_transcript(")
            .count(),
        1,
        "the coordinator must select the product transcript exactly once"
    );
    assert_eq!(
        coordinator
            .matches("product final sealed authority=")
            .count(),
        1,
        "the selected owner content must be sealed exactly once"
    );
    for legacy_writer in [
        "select_target_speaker_final(",
        "raw.text = recovered",
        "raw = replayed",
        "empty ASR final recovered from partial preview",
    ] {
        assert!(
            !coordinator.contains(legacy_writer),
            "legacy downstream transcript writer returned: {legacy_writer}"
        );
    }
    let product_arbiter = include_str!("dictation_preview.rs");
    assert!(product_arbiter.contains("ProductFinalCandidates"));
    assert!(product_arbiter.contains("target_filter_required"));
    assert!(product_arbiter.contains("local_shadow_eligible"));
}

#[test]
fn automatic_wake_target_speaker_endpoint_body_wait_has_bounded_wall_clock_escape() {
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    arm_automatic_wake_text_guard(&coordinator.inner, session_id, "开始录音".into(), 1_200);
    acknowledge_automatic_wake_capsule_visible(&coordinator.inner, session_id);
    let started_at = coordinator
        .inner
        .embedded_audio_automatic_wake_guard
        .lock()
        .as_ref()
        .and_then(|guard| guard.initial_body_wait_started_at)
        .expect("visible capsule must arm wall-clock body wait");

    assert!(super::automatic_wake_initial_body_wait_active_at(
        &coordinator.inner,
        session_id,
        Some(1_200),
        started_at + Duration::from_millis(7_999),
    ));
    assert!(!super::automatic_wake_initial_body_wait_active_at(
        &coordinator.inner,
        session_id,
        Some(1_200),
        started_at + Duration::from_millis(8_000),
    ));
}

#[test]
fn automatic_wake_target_speaker_endpoint_early_capsule_ack_cannot_be_lost() {
    // Live session 777911d9 showed the early Recording capsule before owner
    // acceptance. Installing the accepted-session guard afterward waited for
    // a second visibility ACK that the already-visible frontend never sent,
    // leaving automatic_body_initial_wait active forever.
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    arm_accepted_automatic_wake_text_guard(
        &coordinator.inner,
        session_id,
        "开始录音".into(),
        1_800,
        true,
    );
    let guard = coordinator
        .inner
        .embedded_audio_automatic_wake_guard
        .lock()
        .clone()
        .expect("accepted automatic guard");
    assert!(guard.initial_body_wait_until_audio_ms.is_some());
    assert!(guard.initial_body_wait_started_at.is_some());

    filter_automatic_wake_text(&coordinator.inner, session_id, "开始录音这是正文", true);
    assert!(!automatic_wake_initial_body_wait_active(
        &coordinator.inner,
        session_id,
        Some(1_900),
    ));
}

#[test]
fn target_speaker_endpoint_candidate_capsule_cannot_create_or_close_product_session() {
    let source = include_str!("dictation.rs");
    let start = source
        .find("fn show_early_wake_recording_capsule")
        .expect("candidate capsule helper");
    let end = source[start..]
        .find("fn take_early_capsule_session_id")
        .map(|offset| start + offset)
        .expect("candidate capsule helper boundary");
    let body = &source[start..end];
    assert!(!body.contains("begin_session_state("));
    assert!(!body.contains("state.phase ="));
    assert!(body.contains("uuid::Uuid::new_v4()"));
}

#[test]
fn automatic_wake_target_speaker_endpoint_missing_capsule_ack_is_bounded() {
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    arm_automatic_wake_text_guard(&coordinator.inner, session_id, "开始录音".into(), 1_200);
    let started_at = coordinator
        .inner
        .embedded_audio_automatic_wake_guard
        .lock()
        .as_ref()
        .and_then(|guard| guard.initial_body_wait_started_at)
        .expect("guard arm must start bounded wall escape before frontend ACK");
    assert!(!super::automatic_wake_initial_body_wait_active_at(
        &coordinator.inner,
        session_id,
        Some(1_200),
        started_at + Duration::from_millis(8_000),
    ));
}

#[test]
fn terminal_verified_owner_without_phrase_cannot_start_formal_dictation() {
    // Firmware `VoiceActivation` is the name of a VAD-opened PCM transport
    // window. Sessions such as 1930 reached terminal fallback with
    // host_phrase_detectors=none and were nevertheless relabelled as
    // KeywordModel solely because the owner voiceprint matched. That bypass
    // alternated false wake, slow terminal wake and wake rejection.
    let source = include_str!("dictation_embedded_stream.rs");
    assert!(!source.contains("terminal firmware VoiceActivation fallback accepted"));
    assert!(!source.contains("live firmware VoiceActivation plus enrolled owner accepted"));
    assert!(!source.contains("host_phrase_detectors=none"));

    let arbitration = crate::speech_decision_kernel::arbitrate_wake(
        denzic_voice_activation_v1_core::PhraseSignal::None,
        crate::speech_decision_kernel::OwnerAccessEvidence::EnrolledMatch,
        true,
    );
    assert_eq!(
        arbitration.decision,
        denzic_voice_activation_v1_core::GateDecision::Reject,
        "2026-09-13 user clarification: owner speech without a wake phrase must not start formal recording or emit text"
    );
}

#[test]
fn late_text_after_stop_preserves_body_from_the_same_recording() {
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    arm_automatic_wake_text_guard(&coordinator.inner, session_id, "开始录音".into(), 0);

    // This mirrors the real stop boundary: UI enters Transcribing while the
    // provider may still deliver a final frame on the same session.
    mark_automatic_wake_stop_requested(&coordinator.inner, session_id);
    assert_eq!(
        filter_automatic_wake_text(
            &coordinator.inner,
            session_id,
            "开始录音。正文在停止边界后才到达。",
            false,
        ),
        "正文在停止边界后才到达。"
    );
    assert!(
        automatic_wake_body_started(&coordinator.inner, session_id),
        "2026-09-11 no-word-loss contract retains late body instead of sealing an empty result"
    );
}

#[test]
fn visible_preview_strips_wake_and_replaces_the_wake_only_wait() {
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    arm_automatic_wake_text_guard(
        &coordinator.inner,
        session_id,
        "开始录音".to_string(),
        1_200,
    );

    assert_eq!(
        filter_dictation_visual_preview_text(
            &coordinator.inner,
            session_id,
            "开始录音。这是尚未确认的胶囊临时预览。",
        ),
        "这是尚未确认的胶囊临时预览。"
    );
    assert!(
        automatic_wake_body_started(&coordinator.inner, session_id),
        "shown body is retained and must not expire as an empty wake-only session"
    );
    assert!(current_embedded_audio_partial_preview(&coordinator.inner).is_none());
}

#[test]
fn automatic_wake_preserves_first_clause_after_supported_body_pauses() {
    for pause_ms in [0_u64, 500, 1_000, 2_000, 2_500] {
        let coordinator = Coordinator::new();
        let session_id = new_session_id();
        arm_automatic_wake_text_guard(&coordinator.inner, session_id, "开始录音".into(), 1_200);
        acknowledge_automatic_wake_capsule_visible(&coordinator.inner, session_id);

        assert!(automatic_wake_initial_body_wait_active(
            &coordinator.inner,
            session_id,
            Some(1_200 + pause_ms),
        ));
        assert_eq!(
            filter_automatic_wake_text(
                &coordinator.inner,
                session_id,
                "开始录音。主讲人第一句内容要保持完整。最后这句话也不能丢。",
                true,
            ),
            "主讲人第一句内容要保持完整。最后这句话也不能丢。",
            "pause_ms={pause_ms}"
        );
        assert!(automatic_wake_body_started(&coordinator.inner, session_id));
        assert!(!automatic_wake_initial_body_wait_active(
            &coordinator.inner,
            session_id,
            Some(1_200 + pause_ms),
        ));
        clear_automatic_wake_text_guard(&coordinator.inner);
    }
}

#[test]
fn wake_only_expiry_is_handled_before_empty_transcript_failure_history() {
    let source = include_str!("dictation.rs");
    let branch_start = source
        .find("let wake_only_expired = automatic_wake_session_active")
        .expect("wake-only empty-result branch");
    let branch_tail = &source[branch_start..];
    let silent_return = branch_tail
        .find("return Ok(());")
        .expect("wake-only branch returns before generic failure");
    let empty_failure = branch_tail
        .find("error_code: Some(\"emptyTranscript\".to_string())")
        .expect("generic empty-transcript failure remains after wake-only handling");
    assert!(silent_return < empty_failure);
    assert!(branch_tail[..silent_return].contains("error_code: None"));
    assert!(branch_tail[..silent_return].contains("publish_embedded_ble_wake_only_expired"));
}

#[test]
fn empty_transcript_history_keeps_the_archived_recording_session_id() {
    let source = include_str!("dictation.rs");
    let branch_start = source
        .find("let wake_only_expired = automatic_wake_session_active")
        .expect("empty-transcript branch exists");
    let branch_tail = &source[branch_start..];
    let history_start = branch_tail
        .find("let session = DictationSession {")
        .expect("empty-transcript history session exists");
    let history_tail = &branch_tail[history_start..];
    let history_end = history_tail
        .find("};")
        .expect("empty-transcript history session closes");
    let history = &history_tail[..history_end];

    assert!(history.contains("id: current_session_id.to_string()"));
    assert!(!history.contains("id: Uuid::new_v4().to_string()"));
}

#[test]
fn short_empty_recovery_never_broadens_the_automatic_wake_gate() {
    let source = include_str!("dictation.rs");
    let retry_start = source
        .find("let automatic_wake = automatic_wake_session_active")
        .expect("empty-final recovery gate exists");
    let retry_tail = &source[retry_start..];
    let retry_end = retry_tail
        .find("asr.cancel();")
        .expect("empty-final recovery dispatch exists");
    let gate = &retry_tail[..retry_end];

    assert!(gate.contains("asr.has_sustained_local_speech_evidence()"));
    assert!(gate.contains("!automatic_wake && asr.has_local_speech_evidence()"));
    assert!(retry_tail.contains("replay_retained_audio_once_for_empty_final()"));
}

#[test]
fn automatic_wake_starts_initial_body_wait_at_visible_capsule_ack() {
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    arm_automatic_wake_text_guard(&coordinator.inner, session_id, "开始录音".into(), 1_200);

    // Before visible ack, wait is force-active (deadline not armed yet).
    assert!(automatic_wake_initial_body_wait_active(
        &coordinator.inner,
        session_id,
        Some(1_200)
    ));
    assert_eq!(
        filter_automatic_wake_text(
            &coordinator.inner,
            session_id,
            "开始录音。今天继续测试。",
            true,
        ),
        "今天继续测试。"
    );
    // Real body replaces the wake-only wait even before the UI acknowledgement.
    assert!(!automatic_wake_initial_body_wait_active(
        &coordinator.inner,
        session_id,
        Some(1_300)
    ));
    acknowledge_automatic_wake_capsule_visible(&coordinator.inner, session_id);
    // First non-empty body ends wait immediately after ack.
    assert!(!automatic_wake_initial_body_wait_active(
        &coordinator.inner,
        session_id,
        Some(1_300)
    ));

    let no_body_session_id = new_session_id();
    arm_automatic_wake_text_guard(
        &coordinator.inner,
        no_body_session_id,
        "开始录音".into(),
        1_200,
    );
    assert!(automatic_wake_initial_body_wait_active(
        &coordinator.inner,
        no_body_session_id,
        Some(1_200)
    ));
    acknowledge_automatic_wake_capsule_visible(&coordinator.inner, no_body_session_id);
    // Deadline = capsule_audio 1200 + body wait 8000 = 9200.
    assert!(automatic_wake_initial_body_wait_active(
        &coordinator.inner,
        no_body_session_id,
        Some(9_199)
    ));
    assert!(!automatic_wake_initial_body_wait_active(
        &coordinator.inner,
        no_body_session_id,
        Some(9_200)
    ));

    let manual_session_id = new_session_id();
    assert!(!automatic_wake_initial_body_wait_active(
        &coordinator.inner,
        manual_session_id,
        Some(0)
    ));
}

#[test]
fn long_ambient_wake_candidate_rotates_with_overlap_until_phrase_hit() {
    assert_eq!(
        super::rolling_kws_rotation_start(2_399 * 32, 0, false),
        None
    );
    assert_eq!(
        super::rolling_kws_rotation_start(2_400 * 32, 0, false),
        Some(1_000 * 32)
    );
    assert_eq!(
        super::rolling_kws_rotation_start(3_400 * 32, 1_000 * 32, false),
        Some(2_000 * 32)
    );
    assert_eq!(
        super::rolling_kws_rotation_start(4_500 * 32, 1_500 * 32, true),
        None
    );
}

#[test]
fn rolling_wake_match_keeps_absolute_candidate_boundary() {
    let found = crate::wake_phrase::Match {
        start_seconds: Some(0.10),
        end_seconds: 0.75,
        matched_keyword: Some("开始录音".into()),
    };
    let adjusted = super::offset_streaming_wake_match(Some(found), 1_500 * 32)
        .expect("rolling detector match");
    assert!((adjusted.start_seconds.expect("keyword start") - 1.60).abs() < f32::EPSILON);
    assert!((adjusted.end_seconds - 2.25).abs() < f32::EPSILON);
}

#[test]
fn rolling_local_confirmation_restarts_the_800ms_ladder_per_window() {
    let origin = 1_000 * 32;
    // Local confirmation never follows KWS rotation. Fast pre-roll can burn
    // both 0.8 s and 1.8 s rungs in one second; following origin to ~1 s cuts
    // 开始录音 out of the window.
    assert!(!super::should_advance_local_confirmation_window(
        true, false, 0, origin, 0, false
    ));
    assert!(!super::should_advance_local_confirmation_window(
        true, false, 0, origin, 1, false
    ));
    assert!(!super::should_advance_local_confirmation_window(
        true, false, 0, origin, 2, false
    ));
    assert!(!super::should_advance_local_confirmation_window(
        true, true, 0, origin, 1, false
    ));
    assert!(!super::should_advance_local_confirmation_window(
        true,
        false,
        origin,
        2_000 * 32,
        0,
        false
    ));
    assert!(!super::should_advance_local_confirmation_window(
        true, false, 0, origin, 1, true
    ));
    assert_eq!(
        super::local_confirmation_snapshot_for_window(1_799 * 32, origin, 0),
        None
    );
    assert_eq!(
        super::local_confirmation_snapshot_for_window(1_800 * 32, origin, 0),
        Some(800 * 32)
    );
}

#[test]
fn leading_quiet_prefix_skips_only_bounded_firmware_pre_roll() {
    let quiet = vec![0u8; 400 * 32];
    assert_eq!(super::leading_quiet_prefix_bytes(&quiet), 400 * 32);
    let mut speech = vec![0u8; 200 * 32];
    let peak = 2_000i16.to_le_bytes();
    speech.extend_from_slice(&peak);
    speech.extend_from_slice(&vec![0u8; 200 * 32]);
    assert_eq!(super::leading_quiet_prefix_bytes(&speech), 200 * 32);
}

#[cfg(target_os = "windows")]
#[test]
fn fast_preroll_defers_only_the_first_exploratory_local_confirmation() {
    assert!(
        super::should_defer_exploratory_local_confirmation_for_fast_preroll(
            false,
            0,
            800 * 32,
            Duration::from_millis(100),
        )
    );
    // 2026-09-22 tke:首看 defer 2.4s→1.2s(kws_hit=false 唤醒走探测路径,
    // 首看被 defer 到 2.4s PCM 才是胶囊慢的真瓶颈;后续档位本来就背靠背)。
    // elapsed=500ms 与 ≥2x 快放期一致(谓词还要求 elapsed*2 < pcm_ms)。
    assert!(
        super::should_defer_exploratory_local_confirmation_for_fast_preroll(
            false,
            0,
            1_199 * 32,
            Duration::from_millis(500),
        )
    );
    assert!(
        !super::should_defer_exploratory_local_confirmation_for_fast_preroll(
            false,
            0,
            1_200 * 32,
            Duration::from_millis(500),
        )
    );
    assert!(
        !super::should_defer_exploratory_local_confirmation_for_fast_preroll(
            false,
            0,
            800 * 32,
            Duration::from_millis(500),
        )
    );
    assert!(
        !super::should_defer_exploratory_local_confirmation_for_fast_preroll(
            true,
            0,
            800 * 32,
            Duration::from_millis(100),
        )
    );
    assert!(
        !super::should_defer_exploratory_local_confirmation_for_fast_preroll(
            false,
            1,
            800 * 32,
            Duration::from_millis(100),
        )
    );
}

#[test]
fn ambient_speech_bounds_each_window_but_never_disables_late_phrase_confirmation() {
    assert!(super::exploratory_local_confirmation_allowed(
        false, 0, 0, 0
    ));
    assert!(super::exploratory_local_confirmation_allowed(
        false, 2, 0, 2
    ));
    assert!(super::exploratory_local_confirmation_allowed(
        false, 3, 0, 3
    ));
    assert!(
        super::exploratory_local_confirmation_allowed(false, 4, 0, 4),
        "four early Absents must not disable the 3s/5s phrase-first rungs"
    );
    // Every rolling window gets exactly one focused retry regardless of older
    // candidate-wide Absents; repeated work inside that window remains blocked.
    assert!(super::exploratory_local_confirmation_allowed(
        false,
        3,
        1_040 * 32,
        0
    ));
    assert!(super::exploratory_local_confirmation_allowed(
        false,
        4,
        1_040 * 32,
        0
    ));
    assert!(!super::exploratory_local_confirmation_allowed(
        false,
        4,
        1_040 * 32,
        1
    ));
    assert!(super::exploratory_local_confirmation_allowed(
        false,
        u8::MAX,
        2_040 * 32,
        0
    ));
    assert!(super::exploratory_local_confirmation_allowed(
        true,
        u8::MAX,
        0,
        usize::MAX
    ));
}

#[test]
fn rolling_local_confirmation_discards_only_stale_exploratory_tasks() {
    let old_origin = 1_000 * 32;
    let current_origin = 2_000 * 32;
    assert!(super::local_confirmation_task_is_stale(
        old_origin,
        current_origin,
        false
    ));
    assert!(!super::local_confirmation_task_is_stale(
        current_origin,
        current_origin,
        false
    ));
    assert!(!super::local_confirmation_task_is_stale(
        old_origin,
        current_origin,
        true
    ));

    let exact = super::LocalWakeConfirmation {
        matched: true,
        phrase_relation: crate::wake_phrase::LocalPhraseRelation::ExactStart,
        transcript_chars: 4,
        phonetic_prefix_units: 4,
        phonetic_suffix_units: 0,
        phonetic_best_distance: 0,
        phonetic_best_window_start: 0,
        inference_ms: 200,
        snapshot_pcm_ms: 2_000,
        recovered_keyword_end_seconds: Some(0.8),
    };
    assert!(super::stale_local_confirmation_can_activate(
        true, &exact, false
    ));

    let present_later = super::LocalWakeConfirmation {
        phrase_relation: crate::wake_phrase::LocalPhraseRelation::PresentLater,
        ..exact
    };
    // 2026-09-20: PresentLater no longer waits for a KWS hit, so a rotated
    // window that contained the phrase is an activation-grade positive.
    assert!(super::stale_local_confirmation_can_activate(
        true,
        &present_later,
        false
    ));

    let absent = super::LocalWakeConfirmation {
        matched: false,
        phrase_relation: crate::wake_phrase::LocalPhraseRelation::Absent,
        ..present_later
    };
    assert!(!super::stale_local_confirmation_can_activate(
        true, &absent, false
    ));
}

#[test]
fn automatic_completion_must_receive_provider_final_and_owner_filter_result() {
    let source = include_str!("dictation.rs");
    assert!(!source.contains("release_active_asr_without_sealed_final"),
        "automatic completion cannot cancel a provider that still owes the final transcript and speaker result");
    assert!(!source.contains("let raw = if let Some(preview_text) = auto_end_preview"));
}

#[test]
fn stop_feedback_preserves_queued_pcm_and_partial_tail_until_device_completion() {
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    {
        let mut state = coordinator.inner.state.lock();
        state.session_id = session_id;
        state.phase = SessionPhase::Listening;
        state.cancelled = false;
    }
    let consumer = Arc::new(CapturingConsumer::default());
    let mut session = embedded_audio_test_session(session_id, consumer.clone());
    let prefix = pcm_from_samples(&samples_for_ms(600, 1_000));
    let tail = pcm_from_samples(&samples_for_ms(1427, 3_000));
    session
        .consume_streaming_pcm(&coordinator.inner, &prefix, None)
        .unwrap();
    assert!(request_embedded_audio_stop_feedback(
        &coordinator.inner,
        "captured_tail_test"
    ));
    for packet in tail.chunks(320) {
        session
            .consume_streaming_pcm(&coordinator.inner, packet, None)
            .unwrap();
    }
    session.flush_streaming_pcm();
    let expected = [prefix, tail].concat();
    let submitted = consumer.chunks.lock().unwrap().concat();
    assert_eq!(
        submitted, expected,
        "all pre-STOP capture must reach ASR despite immediate processing feedback"
    );
    assert_eq!(session.archive_pcm.as_deref(), Some(expected.as_slice()));
    assert_eq!(session.normalized_pcm_bytes, expected.len());
}

#[test]
fn embedded_streaming_pcm_flushes_final_partial_block_once() {
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    {
        let mut state = coordinator.inner.state.lock();
        state.session_id = session_id;
        state.phase = SessionPhase::Listening;
        state.cancelled = false;
    }
    let consumer = Arc::new(CapturingConsumer::default());
    let consumer_for_session: Arc<dyn crate::recorder::AudioConsumer> = consumer.clone();
    let mut session = embedded_audio_test_session(session_id, consumer_for_session);
    let tail_pcm = pcm_from_samples(&samples_for_ms(50, 3_000));

    session
        .consume_streaming_pcm(&coordinator.inner, &tail_pcm, None)
        .expect("partial tail is accepted");
    assert!(consumer.chunks.lock().expect("capture lock").is_empty());

    session.flush_streaming_pcm();
    session.flush_streaming_pcm();

    let chunks = consumer.chunks.lock().expect("capture lock");
    assert_eq!(chunks.as_slice(), [tail_pcm]);
    assert_eq!(
        session.normalized_pcm_bytes,
        EMBEDDED_AUDIO_FEED_CHUNK_BYTES / 2
    );
    assert!(session.streaming_pcm_buffer.is_empty());
}

#[test]
fn volcengine_streaming_agc_resolves_one_provider_block_not_each_ble_packet() {
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    {
        let mut state = coordinator.inner.state.lock();
        state.session_id = session_id;
        state.phase = SessionPhase::Listening;
        state.cancelled = false;
    }
    let consumer = Arc::new(CountingConsumer::default());
    let consumer_for_session: Arc<dyn crate::recorder::AudioConsumer> = consumer.clone();
    let mut session = embedded_audio_test_session(session_id, consumer_for_session);
    session.active_asr = "volcengine".into();
    let packet = pcm_from_samples(&samples_for_ms(20, 320));

    for _ in 0..4 {
        session
            .consume_streaming_pcm(&coordinator.inner, &packet, None)
            .expect("short voiced packet is accepted");
    }
    assert_eq!(consumer.bytes.load(Ordering::SeqCst), 0);
    assert_eq!(session.streaming_agc.voiced_chunks, 0);

    session
        .consume_streaming_pcm(&coordinator.inner, &packet, None)
        .expect("provider block is accepted");
    assert_eq!(
        consumer.bytes.load(Ordering::SeqCst),
        EMBEDDED_AUDIO_FEED_CHUNK_BYTES
    );
    assert_eq!(session.streaming_agc.voiced_chunks, 1);
    assert_eq!(session.streaming_agc.first_voiced_pcm_ms, Some(0));
}

#[test]
fn embedded_streaming_counter_audit_separates_physical_segments_from_logical_session() {
    let coordinator = Coordinator::new();
    let coordinator_session_id = new_session_id();
    {
        let mut state = coordinator.inner.state.lock();
        state.session_id = coordinator_session_id;
        state.phase = SessionPhase::Listening;
        state.cancelled = false;
    }

    let first_segment_pcm = pcm_from_samples(&[101, -101, 202, -202]);
    let second_segment_pcm = pcm_from_samples(&[303, -303, 404, -404, 505, -505]);
    let mut collector = crate::embedded_audio::StreamingSessionCollector::default();

    let collect_segment = |collector: &mut crate::embedded_audio::StreamingSessionCollector,
                           embedded_session_id: u32,
                           pcm: &[u8]| {
        collector.reset();
        assert!(matches!(
            collector
                .handle_notification(&build_session_start_notification(embedded_session_id))
                .expect("segment start"),
            StreamingSessionEvent::Started { .. }
        ));
        assert!(matches!(
            collector
                .handle_notification(
                    &build_audio_data_notification(embedded_session_id, 0, pcm)
                        .expect("segment audio")
                )
                .expect("segment audio"),
            StreamingSessionEvent::PcmChunk(_)
        ));
        assert!(matches!(
            collector
                .handle_notification(&build_session_stop_notification(embedded_session_id, 1))
                .expect("segment stop"),
            StreamingSessionEvent::Stopped { .. }
        ));
        collector.inner().stats()
    };

    // A reset is a physical collector/firmware-segment boundary. Its stats
    // describe only the segment currently retained by the collector.
    let first_stats = collect_segment(&mut collector, 701, &first_segment_pcm);
    let second_stats = collect_segment(&mut collector, 702, &second_segment_pcm);
    assert_eq!(first_stats.session_id, Some(701));
    assert_eq!(second_stats.session_id, Some(702));
    assert_eq!(first_stats.reconstructed_pcm_bytes, first_segment_pcm.len());
    assert_eq!(
        second_stats.reconstructed_pcm_bytes,
        second_segment_pcm.len()
    );
    assert_eq!(first_stats.asr_boundary_pcm_bytes, first_segment_pcm.len());
    assert_eq!(
        second_stats.asr_boundary_pcm_bytes,
        second_segment_pcm.len()
    );

    // The coordinator session is a separate owner and can span those physical
    // segments. This is the exact composition that must be visible in future
    // per-segment trace evidence; it is not a claim about old trace 959.
    let consumer = Arc::new(CapturingConsumer::default());
    let consumer_for_session: Arc<dyn crate::recorder::AudioConsumer> = consumer.clone();
    let mut session = embedded_audio_test_session(coordinator_session_id, consumer_for_session);
    session
        .consume_streaming_pcm(&coordinator.inner, &first_segment_pcm, None)
        .expect("first physical segment PCM");
    session
        .consume_streaming_pcm(&coordinator.inner, &second_segment_pcm, None)
        .expect("second physical segment PCM");
    session.flush_streaming_pcm();

    let mut expected_pcm = first_segment_pcm.clone();
    expected_pcm.extend_from_slice(&second_segment_pcm);
    assert_eq!(session.streamed_pcm_bytes, expected_pcm.len());
    assert_eq!(session.normalized_pcm_bytes, expected_pcm.len());
    assert_eq!(
        session.archive_pcm.as_deref(),
        Some(expected_pcm.as_slice())
    );
    assert_eq!(
        consumer.chunks.lock().expect("capture lock").concat(),
        expected_pcm
    );
    assert_eq!(
        first_stats.reconstructed_pcm_bytes + second_stats.reconstructed_pcm_bytes,
        session.streamed_pcm_bytes
    );
}

#[test]
fn embedded_streaming_collector_audit_rejects_duplicate_and_late_foreign_packets() {
    let mut collector = crate::embedded_audio::StreamingSessionCollector::default();
    let first_pcm = [1u8, 2, 3, 4];
    let second_pcm = [5u8, 6, 7, 8];

    collector
        .handle_notification(&build_session_start_notification(801))
        .expect("first segment start");
    collector
        .handle_notification(
            &build_audio_data_notification(801, 0, &first_pcm).expect("first segment audio"),
        )
        .expect("first segment audio");
    assert!(matches!(
        collector
            .handle_notification(
                &build_audio_data_notification(801, 0, &first_pcm)
                    .expect("duplicate audio notification"),
            )
            .expect("duplicate audio notification"),
        StreamingSessionEvent::Ignored(
            crate::embedded_audio::IgnoredPacketReason::DuplicateOrShorterPacket
        )
    ));
    collector
        .handle_notification(&build_session_stop_notification(801, 1))
        .expect("first segment stop");
    let first_stats = collector.inner().stats();
    assert_eq!(first_stats.received_packet_count, 1);
    assert_eq!(first_stats.duplicate_packet_count, 1);
    assert_eq!(first_stats.reconstructed_pcm_bytes, first_pcm.len());

    // The new physical segment owns the collector after rotation. A delayed
    // packet from the old segment must be an Ignored event, never a PCM event.
    collector.reset();
    collector
        .handle_notification(&build_session_start_notification(802))
        .expect("second segment start");
    assert!(matches!(
        collector
            .handle_notification(
                &build_audio_data_notification(801, 1, &first_pcm).expect("late old-segment audio"),
            )
            .expect("late old-segment audio"),
        StreamingSessionEvent::Ignored(crate::embedded_audio::IgnoredPacketReason::ForeignSession)
    ));
    collector
        .handle_notification(
            &build_audio_data_notification(802, 0, &second_pcm).expect("second segment audio"),
        )
        .expect("second segment audio");
    collector
        .handle_notification(&build_session_stop_notification(802, 1))
        .expect("second segment stop");

    // STOP tail remains attributable to the same physical session and is
    // marked for diagnostics, while the coordinator's actor decides whether
    // it is still ASR input.
    let tail = [9u8, 10, 11, 12];
    assert!(matches!(
        collector
            .handle_notification(
                &build_audio_data_notification(802, 1, &tail).expect("same-segment STOP tail"),
            )
            .expect("same-segment STOP tail"),
        StreamingSessionEvent::PcmChunk(ref chunk) if chunk.after_stop_boundary
    ));
    let second_stats = collector.inner().stats();
    assert_eq!(second_stats.session_id, Some(802));
    assert_eq!(second_stats.received_packet_count, 2);
    assert_eq!(second_stats.ignored_foreign_packet_count, 1);
    assert_eq!(second_stats.post_stop_packet_count, 1);
    assert_eq!(second_stats.post_stop_pcm_bytes, tail.len());
    assert_eq!(second_stats.asr_boundary_pcm_bytes, second_pcm.len());
}

#[tokio::test]
async fn embedded_streaming_actor_does_not_process_queued_replacement_during_provider_final_wait() {
    assert!(
        !run_embedded_streaming_final_wait_probe(
            true,
            Ok(crate::asr::RawTranscript {
                text: String::new(),
                duration_ms: 0,
            }),
        )
        .await
    );
}

fn embedded_streaming_final_wait_probe_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

#[tokio::test]
async fn embedded_streaming_source_integrity_blocks_provider_error_before_replay() {
    assert!(
        !run_embedded_streaming_final_wait_probe(
            false,
            Err(
                crate::asr::volcengine::VolcengineASRError::ConnectionFailed(
                    "test primary provider failure".into(),
                )
            ),
        )
        .await
    );
}

#[tokio::test]
async fn voice_activation_candidate_owns_capture_dependency_before_promotion() {
    let coordinator = Coordinator::new();
    let mut streaming = EmbeddedStreamingDictation::background_listener();
    let embedded_session_id = 902_u32;
    let capture_generation = 4_902_u64;
    let mut capture_collector = crate::embedded_audio::SessionCollector::default();

    let start = crate::embedded_audio::build_session_start_notification_with_origin(
        embedded_session_id,
        crate::embedded_audio::SessionStartOrigin::VoiceActivation,
    );
    capture_collector
        .handle_notification(&start)
        .expect("capture candidate start");
    streaming
        .handle_notification_with_capture_generation_and_admission(
            &coordinator.inner,
            &start,
            Some(capture_generation),
            None,
            capture_collector.last_admission_fact(),
            capture_collector.last_admission_receipt(),
        )
        .await
        .expect("actor candidate start");

    let audio =
        build_audio_data_notification(embedded_session_id, 0, &[1, 2]).expect("candidate audio");
    capture_collector
        .handle_notification(&audio)
        .expect("capture candidate audio");
    let capture_receipt = capture_collector
        .last_admission_receipt()
        .expect("capture candidate receipt");
    streaming
        .handle_notification_with_capture_generation_and_admission(
            &coordinator.inner,
            &audio,
            Some(capture_generation),
            None,
            capture_collector.last_admission_fact(),
            Some(capture_receipt.clone()),
        )
        .await
        .expect("actor candidate audio");

    let candidate = streaming
        .speaker_candidate
        .as_mut()
        .expect("voice activation candidate remains gated");
    let ledger = candidate
        .source_admission_ledger
        .lock()
        .expect("candidate source admission ledger lock");
    let dependency = ledger
        .dependencies
        .iter()
        .find(|dependency| dependency.use_kind == super::SourceAdmissionUse::CandidateGateInput)
        .expect("candidate gate dependency");
    assert_eq!(dependency.accepted_bytes, 2);
    let dependency_snapshot = ledger
        .snapshots()
        .into_iter()
        .find(|snapshot| snapshot.use_kind == super::SourceAdmissionUse::CandidateGateInput)
        .expect("candidate gate dependency snapshot");
    assert_eq!(dependency_snapshot.owner_ranges.len(), 1);
    assert!(dependency
        .capture_receipt
        .as_ref()
        .expect("candidate dependency receipt")
        .same_reference(&capture_receipt));
    assert_eq!(
        dependency.current_status(),
        super::CaptureAdmissionBindingStatus::MatchedConsumed
    );
    let release_contexts =
        candidate.source_admission_contexts_for_range(Some(crate::observability::CandidateRange {
            start: 0,
            end: 1,
        }));
    assert_eq!(release_contexts.len(), 1);
    assert_eq!(release_contexts[0].1, 0);
    assert_eq!(release_contexts[0].2, 1);
    assert!(release_contexts[0]
        .0
        .as_ref()
        .and_then(|context| context.capture_receipt.as_ref())
        .expect("partial release context receipt")
        .same_reference(&capture_receipt));

    // A partial release is actual consumption, but it cannot be promoted to
    // a definite SessionBodyInput mapping. Related receipts may remain as
    // possible evidence while the accepted bytes stay explicitly unknown.
    drop(ledger);
    let release_owner = new_session_id();
    super::record_candidate_release_source_dependencies(
        candidate,
        Some(crate::observability::CandidateRange { start: 0, end: 2 }),
        Some(crate::observability::PcmRange { start: 0, end: 1 }),
        None,
        2,
        1,
        super::SourceAdmissionOperationOwner::Session {
            session_id: release_owner,
        },
        Some(99),
        &candidate.source_admission_ledger,
    );
    let ledger = candidate
        .source_admission_ledger
        .lock()
        .expect("candidate source admission ledger after partial release");
    assert!(ledger.dependencies.iter().any(|dependency| {
        dependency.use_kind == super::SourceAdmissionUse::SessionBodyInputPossible
            && dependency.accepted_bytes == 1
    }));
    assert!(!ledger.dependencies.iter().any(|dependency| {
        dependency.use_kind == super::SourceAdmissionUse::SessionBodyInput
            && dependency
                .operations
                .iter()
                .any(|operation| operation.operation_id == Some(99))
    }));
    drop(ledger);

    // A complete release spanning two source runs keeps the non-zero middle
    // destination offsets and source intervals independently attributable.
    let second_context = candidate
        .source_runs
        .front()
        .and_then(|run| run.capture_admission_context.clone());
    candidate
        .source_runs
        .push_back(super::BufferedCandidateSourceRun {
            bytes: 2,
            capture_generation: Some(capture_generation),
            segment_id: Some(902),
            candidate_range: Some(crate::observability::CandidateRange { start: 2, end: 4 }),
            collector_metadata: None,
            collector_emitted_range: None,
            capture_admission_context: second_context,
        });
    super::record_candidate_release_source_dependencies(
        candidate,
        Some(crate::observability::CandidateRange { start: 0, end: 4 }),
        Some(crate::observability::PcmRange { start: 10, end: 14 }),
        Some(crate::observability::PcmSourceInterval {
            capture_generation,
            source_stream_id: 1,
            stream_kind: crate::observability::PcmStreamKind::CoordinatorInputPcm,
            mapping: crate::observability::PcmMappingKind::PositionPreserving,
            segment_id: Some(902),
            range: crate::observability::PcmRange {
                start: 100,
                end: 104,
            },
        }),
        4,
        4,
        super::SourceAdmissionOperationOwner::Session {
            session_id: release_owner,
        },
        Some(100),
        &candidate.source_admission_ledger,
    );

    // Full acceptance with a missing source-run tail preserves the proven
    // prefix and emits an explicit unknown gap instead of stretching the last
    // run across it.
    super::record_candidate_release_source_dependencies(
        candidate,
        Some(crate::observability::CandidateRange { start: 0, end: 5 }),
        Some(crate::observability::PcmRange { start: 20, end: 25 }),
        Some(crate::observability::PcmSourceInterval {
            capture_generation,
            source_stream_id: 1,
            stream_kind: crate::observability::PcmStreamKind::CoordinatorInputPcm,
            mapping: crate::observability::PcmMappingKind::PositionPreserving,
            segment_id: Some(902),
            range: crate::observability::PcmRange {
                start: 200,
                end: 205,
            },
        }),
        5,
        5,
        super::SourceAdmissionOperationOwner::Session {
            session_id: release_owner,
        },
        Some(101),
        &candidate.source_admission_ledger,
    );
    let ledger = candidate
        .source_admission_ledger
        .lock()
        .expect("candidate source admission ledger after mapping matrix");
    let body_snapshots = ledger
        .snapshots()
        .into_iter()
        .filter(|snapshot| snapshot.use_kind == super::SourceAdmissionUse::SessionBodyInput)
        .collect::<Vec<_>>();
    assert!(body_snapshots.iter().any(|snapshot| {
        snapshot.accepted_bytes >= 4
            && snapshot
                .owner_ranges
                .contains(&super::SourceAdmissionOwnerRange::Session { start: 10, end: 12 })
            && snapshot
                .owner_ranges
                .contains(&super::SourceAdmissionOwnerRange::Session { start: 12, end: 14 })
    }));
    assert!(ledger.dependencies.iter().any(|dependency| {
        dependency.use_kind == super::SourceAdmissionUse::SessionBodyInputPossible
            && dependency.operations.iter().any(|operation| {
                operation.operation_id == Some(101) && operation.accepted_bytes == 1
            })
    }));
    drop(ledger);

    // Build two independently identified source runs with an overlapping
    // middle. The overlap must be possible evidence for both sources, never a
    // first-run-wins definite mapping.
    let second_audio = build_audio_data_notification(embedded_session_id, 1, &[3, 4])
        .expect("second capture candidate audio");
    capture_collector
        .handle_notification(&second_audio)
        .expect("second capture candidate audio event");
    let second_capture_fact = capture_collector
        .last_admission_fact()
        .expect("second capture fact");
    let second_capture_receipt = capture_collector
        .last_admission_receipt()
        .expect("second capture receipt");
    let mut second_actor_metadata = candidate
        .source_runs
        .front()
        .and_then(|run| run.capture_admission_context.as_ref())
        .and_then(|context| context.actor_chunk_metadata)
        .expect("first actor metadata");
    second_actor_metadata.packet_sequence = 1;
    second_actor_metadata.collector_instance_id = second_capture_fact.collector_instance_id;
    let second_context = super::CaptureAdmissionSourceContext {
        capture_generation: Some(capture_generation),
        capture_fact: Some(second_capture_fact.clone()),
        capture_receipt: Some(second_capture_receipt.clone()),
        actor_fact: Some(second_capture_fact.clone()),
        actor_chunk_metadata: Some(second_actor_metadata),
    };
    let first_context = candidate
        .source_runs
        .front()
        .and_then(|run| run.capture_admission_context.clone())
        .expect("first source context");

    // A known release range can still contain a source run whose candidate
    // coordinates are UNKNOWN. Its receipt must survive as possible evidence
    // without receiving any definite range or extra accepted bytes.
    candidate
        .source_runs
        .push_back(super::BufferedCandidateSourceRun {
            bytes: 2,
            capture_generation: Some(capture_generation),
            segment_id: Some(903),
            candidate_range: None,
            collector_metadata: None,
            collector_emitted_range: None,
            capture_admission_context: Some(second_context.clone()),
        });
    let known_range_unknown_source_ledger = Arc::new(std::sync::Mutex::new(
        super::SourceAdmissionDependencyLedger::default(),
    ));
    super::record_candidate_release_source_dependencies(
        candidate,
        Some(crate::observability::CandidateRange { start: 0, end: 4 }),
        Some(crate::observability::PcmRange { start: 40, end: 44 }),
        Some(crate::observability::PcmSourceInterval {
            capture_generation,
            source_stream_id: 1,
            stream_kind: crate::observability::PcmStreamKind::CoordinatorInputPcm,
            mapping: crate::observability::PcmMappingKind::PositionPreserving,
            segment_id: Some(902),
            range: crate::observability::PcmRange {
                start: 400,
                end: 404,
            },
        }),
        4,
        4,
        super::SourceAdmissionOperationOwner::Session {
            session_id: release_owner,
        },
        Some(400),
        &known_range_unknown_source_ledger,
    );
    let known_range_unknown_source_snapshots = known_range_unknown_source_ledger
        .lock()
        .expect("known-range unknown-source ledger")
        .snapshots();
    assert!(known_range_unknown_source_snapshots.iter().any(|snapshot| {
        snapshot.use_kind == super::SourceAdmissionUse::SessionBodyInputPossible
            && snapshot.capture_admission_id
                == second_context
                    .capture_fact
                    .as_ref()
                    .and_then(|fact| fact.admission_id)
            && snapshot.accepted_bytes == 0
    }));
    assert_eq!(
        known_range_unknown_source_snapshots
            .iter()
            .filter(|snapshot| snapshot.use_kind == super::SourceAdmissionUse::SessionBodyInput)
            .map(|snapshot| snapshot.accepted_bytes)
            .sum::<u64>(),
        4
    );
    candidate.source_runs.clear();
    candidate
        .source_runs
        .push_back(super::BufferedCandidateSourceRun {
            bytes: 4,
            capture_generation: Some(capture_generation),
            segment_id: Some(901),
            candidate_range: Some(crate::observability::CandidateRange { start: 0, end: 4 }),
            collector_metadata: None,
            collector_emitted_range: None,
            capture_admission_context: Some(first_context.clone()),
        });
    candidate
        .source_runs
        .push_back(super::BufferedCandidateSourceRun {
            bytes: 4,
            capture_generation: Some(capture_generation),
            segment_id: Some(902),
            candidate_range: Some(crate::observability::CandidateRange { start: 2, end: 6 }),
            collector_metadata: None,
            collector_emitted_range: None,
            capture_admission_context: Some(second_context.clone()),
        });
    let overlap_ledger = Arc::new(std::sync::Mutex::new(
        super::SourceAdmissionDependencyLedger::default(),
    ));
    super::record_candidate_release_source_dependencies(
        candidate,
        Some(crate::observability::CandidateRange { start: 0, end: 6 }),
        Some(crate::observability::PcmRange { start: 30, end: 36 }),
        Some(crate::observability::PcmSourceInterval {
            capture_generation,
            source_stream_id: 1,
            stream_kind: crate::observability::PcmStreamKind::CoordinatorInputPcm,
            mapping: crate::observability::PcmMappingKind::PositionPreserving,
            segment_id: Some(902),
            range: crate::observability::PcmRange {
                start: 300,
                end: 306,
            },
        }),
        6,
        6,
        super::SourceAdmissionOperationOwner::Session {
            session_id: release_owner,
        },
        Some(200),
        &overlap_ledger,
    );
    let overlap_snapshots = overlap_ledger.lock().expect("overlap ledger").snapshots();
    let mut overlap_ranges = overlap_snapshots
        .iter()
        .filter(|snapshot| snapshot.use_kind == super::SourceAdmissionUse::SessionBodyInput)
        .flat_map(|snapshot| snapshot.owner_ranges.iter().copied())
        .collect::<Vec<_>>();
    overlap_ranges.sort_by_key(|range| match range {
        super::SourceAdmissionOwnerRange::Candidate { start, .. }
        | super::SourceAdmissionOwnerRange::Session { start, .. } => *start,
    });
    assert_eq!(
        overlap_ranges,
        vec![
            super::SourceAdmissionOwnerRange::Session { start: 30, end: 32 },
            super::SourceAdmissionOwnerRange::Session { start: 34, end: 36 },
        ]
    );
    assert!(overlap_snapshots.iter().any(|snapshot| {
        snapshot.use_kind == super::SourceAdmissionUse::SessionBodyInputPossible
            && snapshot.accepted_bytes == 2
    }));

    // Reordering source runs must not change the definite/possible result.
    candidate.source_runs.make_contiguous().swap(0, 1);
    let reordered_overlap_ledger = Arc::new(std::sync::Mutex::new(
        super::SourceAdmissionDependencyLedger::default(),
    ));
    super::record_candidate_release_source_dependencies(
        candidate,
        Some(crate::observability::CandidateRange { start: 0, end: 6 }),
        Some(crate::observability::PcmRange { start: 30, end: 36 }),
        Some(crate::observability::PcmSourceInterval {
            capture_generation,
            source_stream_id: 1,
            stream_kind: crate::observability::PcmStreamKind::CoordinatorInputPcm,
            mapping: crate::observability::PcmMappingKind::PositionPreserving,
            segment_id: Some(902),
            range: crate::observability::PcmRange {
                start: 300,
                end: 306,
            },
        }),
        6,
        6,
        super::SourceAdmissionOperationOwner::Session {
            session_id: release_owner,
        },
        Some(200),
        &reordered_overlap_ledger,
    );
    let mut reordered_overlap_ranges = reordered_overlap_ledger
        .lock()
        .expect("reordered overlap ledger")
        .snapshots()
        .into_iter()
        .filter(|snapshot| snapshot.use_kind == super::SourceAdmissionUse::SessionBodyInput)
        .flat_map(|snapshot| snapshot.owner_ranges)
        .collect::<Vec<_>>();
    reordered_overlap_ranges.sort_by_key(|range| match range {
        super::SourceAdmissionOwnerRange::Candidate { start, .. }
        | super::SourceAdmissionOwnerRange::Session { start, .. } => *start,
    });
    assert_eq!(reordered_overlap_ranges, overlap_ranges);

    // When the candidate coordinate is UNKNOWN, retain both receipt refs as
    // possible sources while counting the accepted operation only once.
    let unknown_coordinate_ledger = Arc::new(std::sync::Mutex::new(
        super::SourceAdmissionDependencyLedger::default(),
    ));
    super::record_candidate_release_source_dependencies(
        candidate,
        None,
        None,
        None,
        2,
        2,
        super::SourceAdmissionOperationOwner::Session {
            session_id: release_owner,
        },
        Some(300),
        &unknown_coordinate_ledger,
    );
    let unknown_snapshots = unknown_coordinate_ledger
        .lock()
        .expect("unknown coordinate ledger")
        .snapshots();
    let unknown_possible = unknown_snapshots
        .iter()
        .filter(|snapshot| snapshot.use_kind == super::SourceAdmissionUse::SessionBodyInputPossible)
        .collect::<Vec<_>>();
    assert!(unknown_possible.iter().any(|snapshot| {
        snapshot.capture_admission_id
            == first_context
                .capture_fact
                .as_ref()
                .and_then(|fact| fact.admission_id)
            && snapshot.accepted_bytes == 0
    }));
    assert!(unknown_possible.iter().any(|snapshot| {
        snapshot.capture_admission_id
            == second_context
                .capture_fact
                .as_ref()
                .and_then(|fact| fact.admission_id)
            && snapshot.accepted_bytes == 0
    }));
    assert_eq!(
        unknown_possible
            .iter()
            .map(|snapshot| snapshot.accepted_bytes)
            .sum::<u64>(),
        2
    );
}

#[tokio::test]
async fn embedded_streaming_actor_empty_final_completes_without_cancellation() {
    assert!(
        !run_embedded_streaming_final_wait_probe(
            false,
            Ok(crate::asr::RawTranscript {
                text: "保留整句的 provider final".into(),
                duration_ms: 125,
            }),
        )
        .await
    );
}

async fn run_embedded_streaming_final_wait_probe(
    cancel_before_final_release: bool,
    provider_result: Result<crate::asr::RawTranscript, crate::asr::volcengine::VolcengineASRError>,
) -> bool {
    let _probe_guard = embedded_streaming_final_wait_probe_lock().lock().await;
    // This is a production-entry concurrency probe, not a second packet
    // admission implementation. Capture and actor deliberately own separate
    // collectors, while the actor invokes the real notification handler.
    struct ActorSignal {
        label: &'static str,
        notification: Vec<u8>,
        observation: Arc<crate::observability::EmbeddedAudioPipelineObservation>,
        capture_admission_fact: Option<crate::embedded_audio::SessionAdmissionFact>,
        capture_admission_receipt: Option<crate::embedded_audio::SessionAdmissionReceipt>,
        ack: tokio::sync::oneshot::Sender<Result<super::EmbeddedBleNotificationAction, String>>,
    }

    let coordinator = Coordinator::new();
    let inner = Arc::clone(&coordinator.inner);
    let coordinator_session_id = new_session_id();
    let embedded_session_id = 901_u32;
    let capture_generation = 4_901_u64;
    {
        let mut state = inner.state.lock();
        state.session_id = coordinator_session_id;
        state.phase = SessionPhase::Listening;
        state.cancelled = false;
    }

    let asr = Arc::new(crate::asr::volcengine::VolcengineStreamingASR::new(
        crate::asr::VolcengineCredentials {
            app_id: "test".into(),
            access_token: "test".into(),
            resource_id: crate::asr::VolcengineCredentials::default_resource_id().into(),
        },
        Vec::new(),
    ));
    asr.mark_audio_delivery_ready();
    let (final_wait_entered, release_final_wait) =
        asr.install_test_final_wait_barrier_with_provider_result(provider_result);
    *inner.asr.lock() = Some(super::SessionResource::new(
        coordinator_session_id,
        super::ActiveAsr::Volcengine(Arc::clone(&asr)),
    ));

    let consumer = Arc::new(CapturingConsumer::default());
    let consumer_for_session: Arc<dyn crate::recorder::AudioConsumer> = consumer.clone();
    let mut session = embedded_audio_test_session(coordinator_session_id, consumer_for_session);
    session.active_asr = "volcengine".into();
    session.volcengine_asr = Some(Arc::clone(&asr));
    let session_source_admission_ledger = Arc::clone(&session.source_admission_ledger);

    let mut streaming = EmbeddedStreamingDictation::background_listener();
    streaming.embedded_session_id = Some(embedded_session_id);
    streaming.session = Some(session);
    assert!(streaming.speaker_candidate.is_none());
    let streaming = Arc::new(tokio::sync::Mutex::new(streaming));
    let cancel_capture = Arc::new(AtomicBool::new(false));
    super::clear_embedded_audio_preview_session(&inner, coordinator_session_id);
    super::clear_embedded_audio_final_result(&inner);
    assert!(super::current_embedded_audio_final_preview_candidate(
        &inner,
        coordinator_session_id,
        0,
    )
    .is_none());
    assert!(
        super::debug_transcript_override_text().is_none(),
        "normal empty-final probe must not use a debug transcript override"
    );
    let observation =
        crate::observability::begin_embedded_audio_pipeline_capture(capture_generation)
            .observation();
    let mut capture_collector = crate::embedded_audio::SessionCollector::default();

    let (signal_tx, mut signal_rx) = tokio::sync::mpsc::unbounded_channel::<ActorSignal>();
    let actor_received = Arc::new(AtomicUsize::new(0));
    let actor_handler_entered = Arc::new(AtomicUsize::new(0));
    let actor_handler_completed = Arc::new(AtomicUsize::new(0));
    let actor_events = Arc::new(Mutex::new(Vec::<&'static str>::new()));
    let actor = {
        let streaming = Arc::clone(&streaming);
        let inner = Arc::clone(&inner);
        let cancel_capture = Arc::clone(&cancel_capture);
        let actor_received = Arc::clone(&actor_received);
        let actor_handler_entered = Arc::clone(&actor_handler_entered);
        let actor_handler_completed = Arc::clone(&actor_handler_completed);
        let actor_events = Arc::clone(&actor_events);
        tokio::spawn(async move {
            while let Some(signal) = signal_rx.recv().await {
                actor_received.fetch_add(1, Ordering::SeqCst);
                let mut streaming = streaming.lock().await;
                actor_handler_entered.fetch_add(1, Ordering::SeqCst);
                actor_events
                    .lock()
                    .expect("actor event lock")
                    .push(signal.label);
                let result = super::process_embedded_ble_notification(
                    &inner,
                    &mut streaming,
                    &signal.notification,
                    capture_generation,
                    Some(signal.observation),
                    signal.capture_admission_fact,
                    signal.capture_admission_receipt,
                    false,
                    &cancel_capture,
                )
                .await;
                actor_handler_completed.fetch_add(1, Ordering::SeqCst);
                let failed = result.is_err();
                if signal.label == "stop" {
                    match &result {
                        Ok(super::EmbeddedBleNotificationAction::ContinueAfterCompletedSession) => {
                            actor_events
                                .lock()
                                .expect("actor event lock")
                                .push("production_completion_branch_returned");
                            assert!(!streaming.terminal_received);
                            assert!(streaming.session.is_none());
                            assert!(streaming.embedded_session_id.is_none());
                            actor_events
                                .lock()
                                .expect("actor event lock")
                                .push("actor_reset_observed");
                        }
                        Ok(super::EmbeddedBleNotificationAction::Continue) => {
                            actor_events
                                .lock()
                                .expect("actor event lock")
                                .push("production_error_branch_returned");
                            assert!(!streaming.terminal_received);
                            assert!(streaming.session.is_none());
                            assert!(streaming.embedded_session_id.is_none());
                            actor_events
                                .lock()
                                .expect("actor event lock")
                                .push("actor_reset_observed");
                        }
                        other => panic!("unexpected STOP action: {other:?}"),
                    }
                } else if signal.label == "replacement" {
                    actor_events
                        .lock()
                        .expect("actor event lock")
                        .push("replacement_processed");
                }
                let _ = signal.ack.send(result);
                if failed {
                    break;
                }
            }
        })
    };

    let start = build_session_start_notification(embedded_session_id);
    let audio = build_audio_data_notification(embedded_session_id, 0, &[1, 2])
        .expect("short valid audio packet");
    let stop = build_session_stop_notification(embedded_session_id, 1);
    let replacement = build_audio_data_notification(embedded_session_id, 0, &[1, 2, 3, 4])
        .expect("larger valid replacement packet");

    for notification in [&start, &audio] {
        let capture_event = capture_collector
            .handle_notification(notification)
            .expect("capture collector accepts initial packet");
        assert!(matches!(
            capture_event,
            crate::embedded_audio::SessionEvent::Started { .. }
                | crate::embedded_audio::SessionEvent::AudioData { .. }
        ));
        let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
        let capture_admission_fact = capture_collector.last_admission_fact();
        let capture_admission_receipt = capture_collector.last_admission_receipt();
        signal_tx
            .send(ActorSignal {
                label: if std::ptr::eq(notification, &start) {
                    "start"
                } else {
                    "audio"
                },
                notification: notification.clone(),
                observation: Arc::clone(&observation),
                capture_admission_fact,
                capture_admission_receipt,
                ack: ack_tx,
            })
            .expect("actor signal is queued");
        let action = tokio::time::timeout(std::time::Duration::from_secs(2), ack_rx)
            .await
            .expect("actor initial ack must not hang")
            .expect("actor initial ack")
            .expect("actor initial packet succeeds");
        assert_eq!(action, super::EmbeddedBleNotificationAction::Continue);
    }

    {
        let ledger = session_source_admission_ledger
            .lock()
            .expect("session source admission ledger lock");
        let body_dependency = ledger
            .dependencies
            .iter()
            .find(|dependency| dependency.use_kind == super::SourceAdmissionUse::SessionBodyInput)
            .expect("initial audio session-owned dependency");
        assert_eq!(body_dependency.accepted_bytes, 2);
        let body_snapshot = ledger
            .snapshots()
            .into_iter()
            .find(|snapshot| snapshot.use_kind == super::SourceAdmissionUse::SessionBodyInput)
            .expect("initial audio session-owned dependency snapshot");
        assert_eq!(
            body_snapshot.owner_ranges,
            vec![super::SourceAdmissionOwnerRange::Session { start: 0, end: 2 }]
        );
        assert!(body_dependency
            .capture_receipt
            .as_ref()
            .expect("session dependency receipt")
            .same_reference(
                &capture_collector
                    .last_admission_receipt()
                    .expect("capture initial receipt")
            ));
        assert_eq!(
            body_dependency.current_status(),
            super::CaptureAdmissionBindingStatus::MatchedConsumed
        );
    }

    // Keep a clone of the actor's initial-audio receipt while the binding is
    // still live. The actor and capture collectors must carry the same Arc,
    // not merely equal witness values.
    let initial_actor_receipt = {
        let streaming = streaming.lock().await;
        streaming
            .capture_admission_bindings
            .iter()
            .find(|binding| {
                binding
                    .capture_fact
                    .as_ref()
                    .and_then(|fact| fact.admission_id)
                    == Some(0)
            })
            .and_then(|binding| binding.capture_receipt.clone())
            .expect("initial actor binding receipt")
    };
    let initial_capture_receipt = capture_collector
        .last_admission_receipt()
        .expect("initial capture receipt");

    let capture_stop = capture_collector
        .handle_notification(&stop)
        .expect("capture collector accepts stop");
    assert!(matches!(
        capture_stop,
        crate::embedded_audio::SessionEvent::Stopped { .. }
    ));

    let entered_wait = final_wait_entered.notified();
    let (stop_ack_tx, mut stop_ack_rx) = tokio::sync::oneshot::channel();
    let capture_admission_fact = capture_collector.last_admission_fact();
    let capture_admission_receipt = capture_collector.last_admission_receipt();
    signal_tx
        .send(ActorSignal {
            label: "stop",
            notification: stop,
            observation: Arc::clone(&observation),
            capture_admission_fact,
            capture_admission_receipt,
            ack: stop_ack_tx,
        })
        .expect("stop signal is queued");
    tokio::time::timeout(std::time::Duration::from_secs(2), entered_wait)
        .await
        .expect("provider final wait must be entered");
    assert_eq!(actor_received.load(Ordering::SeqCst), 3);
    assert_eq!(actor_handler_entered.load(Ordering::SeqCst), 3);
    assert_eq!(actor_handler_completed.load(Ordering::SeqCst), 2);

    let capture_replacement = capture_collector
        .handle_notification(&replacement)
        .expect("capture collector accepts larger same-sequence replacement");
    assert!(matches!(
        capture_replacement,
        crate::embedded_audio::SessionEvent::AudioData { .. }
    ));
    let capture_stats = capture_collector.stats();
    assert_eq!(capture_stats.replaced_packet_count, 1);
    assert_eq!(capture_stats.post_stop_packet_count, 1);

    let (replacement_ack_tx, replacement_ack_rx) = tokio::sync::oneshot::channel();
    let capture_admission_fact = capture_collector.last_admission_fact();
    let capture_replacement_receipt = capture_collector.last_admission_receipt();
    let pending_replacement_admission_id = capture_admission_fact
        .as_ref()
        .expect("replacement capture fact")
        .admission_id;
    let pending_replacement_notification_id = capture_admission_fact
        .as_ref()
        .expect("replacement capture fact")
        .notification_id;
    signal_tx
        .send(ActorSignal {
            label: "replacement",
            notification: replacement,
            observation: Arc::clone(&observation),
            capture_admission_fact,
            capture_admission_receipt: capture_replacement_receipt.clone(),
            ack: replacement_ack_tx,
        })
        .expect("replacement signal is queued");

    // The signal crossed the queue boundary, but the real actor handler is
    // still inside finish_streaming_session -> provider final wait.
    assert_eq!(actor_received.load(Ordering::SeqCst), 3);
    assert_eq!(actor_handler_entered.load(Ordering::SeqCst), 3);
    assert_eq!(actor_handler_completed.load(Ordering::SeqCst), 2);
    assert!(matches!(
        stop_ack_rx.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));
    assert!(initial_actor_receipt.same_reference(&initial_capture_receipt));
    let initial_witness = initial_actor_receipt.witness();
    assert!(initial_witness.superseded);
    assert_eq!(
        initial_witness.superseded_by_admission_id,
        pending_replacement_admission_id
    );
    assert_eq!(
        initial_witness.superseded_by_notification_id,
        pending_replacement_notification_id
    );
    {
        let mut ledger = session_source_admission_ledger
            .lock()
            .expect("session source admission ledger lock");
        let body_dependency = ledger
            .dependencies
            .iter()
            .find(|dependency| dependency.use_kind == super::SourceAdmissionUse::SessionBodyInput)
            .expect("session dependency during final wait");
        assert!(body_dependency
            .capture_receipt
            .as_ref()
            .expect("session dependency receipt during final wait")
            .same_reference(&initial_capture_receipt));
        assert_eq!(
            body_dependency.current_status(),
            super::CaptureAdmissionBindingStatus::MatchedConsumed
        );
        let body_snapshot = ledger
            .snapshots()
            .into_iter()
            .find(|snapshot| snapshot.use_kind == super::SourceAdmissionUse::SessionBodyInput)
            .expect("session dependency snapshot during final wait");
        assert!(!body_snapshot.tracking_incomplete);
        let decision = super::super::source_integrity::qualify_active_source_integrity(
            super::super::source_integrity::SourceIntegrityOwner::Session(coordinator_session_id),
            super::super::source_integrity::SourceIntegrityOwner::Session(coordinator_session_id),
            &mut ledger,
            None,
        );
        assert_eq!(
            decision.verdict,
            super::super::source_integrity::SourceIntegrityQualification::SourceIntegrityBlocked
        );
        assert_eq!(
            decision.evidence_completeness,
            super::super::source_integrity::SourceIntegrityEvidenceCompleteness::Complete
        );
        assert_eq!(
            decision.reason,
            super::super::source_integrity::SourceIntegrityDecisionReason::SessionBodyInputSuperseded
        );
    }

    if cancel_before_final_release {
        // Keep the cancellation probe offline: after the provider final is
        // released, the real stop pipeline would continue into OS input
        // delivery. This is deliberately a cancellation-path guard.
        inner.state.lock().cancelled = true;
        actor_events
            .lock()
            .expect("actor event lock")
            .push("cancellation_set_before_final_release");
    } else {
        assert!(!inner.state.lock().cancelled);
    }
    release_final_wait.notify_waiters();
    let stop_result = tokio::time::timeout(std::time::Duration::from_secs(2), stop_ack_rx)
        .await
        .expect("provider final wait must release and production handler return")
        .expect("stop ack after final wait");
    let stop_action = stop_result.expect("stop handler succeeds");
    assert_eq!(
        stop_action,
        super::EmbeddedBleNotificationAction::ContinueAfterCompletedSession
    );

    if !cancel_before_final_release {
        assert!(!inner.state.lock().cancelled);
        assert!(!asr.has_sustained_local_speech_evidence());
        assert!(!asr.has_local_speech_evidence());
        let history = coordinator.history().list().expect("test history list");
        let session = history
            .iter()
            .find(|session| session.id == coordinator_session_id.to_string())
            .expect("blocked final must be retained in test history exactly once");
        assert_eq!(session.insert_status, InsertStatus::Failed);
        assert_eq!(
            session.error_code.as_deref(),
            Some("source_integrity_blocked")
        );
        assert_eq!(session.raw_transcript, "");
        assert_eq!(session.final_text, "");
        assert_eq!(
            history
                .iter()
                .filter(|session| session.id == coordinator_session_id.to_string())
                .count(),
            1,
            "blocked close must not append duplicate history"
        );
    }

    let replacement_result =
        tokio::time::timeout(std::time::Duration::from_secs(2), replacement_ack_rx)
            .await
            .expect("queued replacement must reach actor after final wait")
            .expect("replacement ack");
    assert_eq!(
        replacement_result.expect("replacement handler result"),
        super::EmbeddedBleNotificationAction::Continue
    );
    assert_eq!(actor_received.load(Ordering::SeqCst), 4);
    assert_eq!(actor_handler_entered.load(Ordering::SeqCst), 4);
    assert_eq!(actor_handler_completed.load(Ordering::SeqCst), 4);
    assert_eq!(
        consumer
            .chunks
            .lock()
            .expect("consumer capture lock")
            .concat(),
        vec![1, 2],
        "late replacement must not enter the old session consumer after reset"
    );

    drop(signal_tx);
    actor.await.expect("actor task");
    let streaming = streaming.lock().await;
    let actor_stats = streaming.collector.inner().stats();
    assert_eq!(actor_stats.replaced_packet_count, 0);
    assert_eq!(actor_stats.received_packet_count, 1);
    assert_eq!(actor_stats.session_id, Some(embedded_session_id));
    assert!(actor_stats.start_inferred_from_audio);
    assert!(!streaming.capture_admission_bindings_incomplete);
    assert_eq!(streaming.capture_admission_bindings.len(), 3);
    let start_binding = &streaming.capture_admission_bindings[0];
    assert_eq!(start_binding.capture_receipt, None);
    assert_eq!(
        start_binding.status,
        super::CaptureAdmissionBindingStatus::CaptureReceiptMissing
    );
    assert!(!start_binding.actor_consumed);
    let first_binding = &streaming.capture_admission_bindings[1];
    let replacement_binding = &streaming.capture_admission_bindings[2];
    let first_capture_fact = first_binding
        .capture_fact
        .as_ref()
        .expect("initial capture fact");
    let replacement_capture_fact = replacement_binding
        .capture_fact
        .as_ref()
        .expect("replacement capture fact");
    assert_eq!(first_capture_fact.admission_id, Some(0));
    assert_eq!(
        first_capture_fact.disposition,
        crate::embedded_audio::SessionAdmissionDisposition::New
    );
    assert_eq!(replacement_capture_fact.admission_id, Some(1));
    assert_eq!(
        replacement_capture_fact.disposition,
        crate::embedded_audio::SessionAdmissionDisposition::Replacement
    );
    assert_eq!(replacement_capture_fact.supersedes_admission_id, Some(0));
    assert_eq!(first_binding.capture_generation, Some(capture_generation));
    assert_eq!(
        replacement_binding.capture_generation,
        Some(capture_generation)
    );
    assert!(
        first_binding
            .capture_receipt
            .as_ref()
            .expect("initial capture receipt")
            .witness()
            .superseded
    );
    assert!(
        !replacement_binding
            .capture_receipt
            .as_ref()
            .expect("replacement capture receipt")
            .witness()
            .superseded
    );
    assert!(first_binding.actor_chunk_metadata.is_some());
    assert!(first_binding.actor_consumed);
    assert_eq!(
        first_binding.status,
        super::CaptureAdmissionBindingStatus::MatchedConsumed
    );
    assert!(replacement_binding.actor_chunk_metadata.is_some());
    assert!(!replacement_binding.actor_consumed);
    assert_eq!(
        replacement_binding.status,
        super::CaptureAdmissionBindingStatus::ActorNotConsumed
    );
    let first_actor_fact = first_binding
        .actor_fact
        .as_ref()
        .expect("initial actor fact");
    let replacement_actor_fact = replacement_binding
        .actor_fact
        .as_ref()
        .expect("replacement actor fact");
    assert_eq!(
        first_actor_fact.disposition,
        crate::embedded_audio::SessionAdmissionDisposition::New
    );
    assert_eq!(
        replacement_actor_fact.disposition,
        crate::embedded_audio::SessionAdmissionDisposition::New
    );
    assert_ne!(
        first_actor_fact.reset_epoch,
        replacement_actor_fact.reset_epoch
    );
    assert_ne!(first_actor_fact.collector_instance_id, Some(0));
    assert_eq!(
        first_actor_fact.collector_instance_id,
        replacement_actor_fact.collector_instance_id
    );
    let expected_events = if cancel_before_final_release {
        vec![
            "start",
            "audio",
            "stop",
            "cancellation_set_before_final_release",
            "production_completion_branch_returned",
            "actor_reset_observed",
            "replacement",
            "replacement_processed",
        ]
    } else {
        vec![
            "start",
            "audio",
            "stop",
            "production_completion_branch_returned",
            "actor_reset_observed",
            "replacement",
            "replacement_processed",
        ]
    };
    assert_eq!(
        actor_events.lock().expect("actor event lock").as_slice(),
        expected_events.as_slice()
    );
    asr.recovery_replay_started_for_test()
}

#[test]
fn capture_admission_binding_rejects_missing_and_mismatched_source_facts() {
    let mut capture_collector = crate::embedded_audio::SessionCollector::default();
    capture_collector
        .handle_notification(
            &crate::embedded_audio::build_audio_data_notification(910, 0, &[1, 2])
                .expect("capture audio"),
        )
        .expect("capture audio event");
    let capture_fact = capture_collector
        .last_admission_fact()
        .expect("capture fact");
    let capture_receipt = capture_collector
        .last_admission_receipt()
        .expect("capture receipt");
    let mut actor_fact = capture_fact.clone();
    actor_fact.collector_instance_id = Some(777);
    actor_fact.reset_epoch = Some(44);
    actor_fact.notification_id = Some(55);
    actor_fact.admission_id = Some(66);
    let actor_metadata = crate::embedded_audio::StreamingPcmChunkMetadata {
        collector_instance_id: Some(888),
        segment_ordinal: 4,
        packet_sequence: 0,
        emission_ordinal: 2,
        emitted_range: crate::embedded_audio::StreamingPcmRange { start: 0, end: 2 },
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

    assert_eq!(
        super::capture_admission_binding_status(
            Some(910),
            Some(&capture_fact),
            Some(&capture_receipt),
            Some(&actor_fact),
            Some(actor_metadata),
            true,
        ),
        super::CaptureAdmissionBindingStatus::MatchedConsumed
    );

    assert_eq!(
        super::capture_admission_binding_status(
            None,
            Some(&capture_fact),
            Some(&capture_receipt),
            Some(&actor_fact),
            Some(actor_metadata),
            true,
        ),
        super::CaptureAdmissionBindingStatus::CaptureGenerationMissing
    );
    assert_eq!(
        super::capture_admission_binding_status(
            Some(910),
            None,
            Some(&capture_receipt),
            Some(&actor_fact),
            Some(actor_metadata),
            true,
        ),
        super::CaptureAdmissionBindingStatus::CaptureFactMissing
    );
    assert_eq!(
        super::capture_admission_binding_status(
            Some(910),
            Some(&capture_fact),
            None,
            Some(&actor_fact),
            Some(actor_metadata),
            true,
        ),
        super::CaptureAdmissionBindingStatus::CaptureReceiptMissing
    );

    let mut mismatched_fact = capture_fact.clone();
    mismatched_fact.packet_sequence = Some(9);
    assert_eq!(
        super::capture_admission_binding_status(
            Some(910),
            Some(&mismatched_fact),
            Some(&capture_receipt),
            Some(&actor_fact),
            Some(actor_metadata),
            true,
        ),
        super::CaptureAdmissionBindingStatus::CaptureReceiptFactMismatch
    );

    let mut foreign_actor_fact = actor_fact.clone();
    foreign_actor_fact.physical_session_id = Some(911);
    assert_eq!(
        super::capture_admission_binding_status(
            Some(910),
            Some(&capture_fact),
            Some(&capture_receipt),
            Some(&foreign_actor_fact),
            Some(actor_metadata),
            true,
        ),
        super::CaptureAdmissionBindingStatus::ActorAdmissionMismatch
    );

    let ignored_actor_fact = crate::embedded_audio::SessionAdmissionFact {
        disposition: crate::embedded_audio::SessionAdmissionDisposition::Ignored(
            crate::embedded_audio::IgnoredPacketReason::ForeignSession,
        ),
        ..actor_fact.clone()
    };
    assert_eq!(
        super::capture_admission_binding_status(
            Some(910),
            Some(&capture_fact),
            Some(&capture_receipt),
            Some(&ignored_actor_fact),
            None,
            false,
        ),
        super::CaptureAdmissionBindingStatus::ActorIgnored
    );
    assert_eq!(
        super::capture_admission_binding_status(
            Some(910),
            Some(&capture_fact),
            Some(&capture_receipt),
            Some(&actor_fact),
            None,
            false,
        ),
        super::CaptureAdmissionBindingStatus::ActorMetadataMissing
    );
    let mut incomplete_metadata = actor_metadata;
    incomplete_metadata.metadata_incomplete = true;
    assert_eq!(
        super::capture_admission_binding_status(
            Some(910),
            Some(&capture_fact),
            Some(&capture_receipt),
            Some(&actor_fact),
            Some(incomplete_metadata),
            true,
        ),
        super::CaptureAdmissionBindingStatus::ActorMetadataIncomplete
    );
    assert_eq!(
        super::capture_admission_binding_status(
            Some(910),
            Some(&capture_fact),
            Some(&capture_receipt),
            Some(&actor_fact),
            Some(actor_metadata),
            false,
        ),
        super::CaptureAdmissionBindingStatus::ActorNotConsumed
    );

    // The owner ledger deduplicates a split admission by shared receipt
    // identity while retaining each proven owner range. Candidate-gate and
    // session-body use are deliberately separate facts.
    let owner_ledger = Arc::new(std::sync::Mutex::new(
        super::SourceAdmissionDependencyLedger::default(),
    ));
    let source_context = super::CaptureAdmissionSourceContext {
        capture_generation: Some(910),
        capture_fact: Some(capture_fact.clone()),
        capture_receipt: Some(capture_receipt.clone()),
        actor_fact: Some(actor_fact.clone()),
        actor_chunk_metadata: Some(actor_metadata),
    };
    let owner_session_id = new_session_id();
    super::record_source_admission_dependency(
        &owner_ledger,
        Some(&source_context),
        super::SourceAdmissionOperationOwner::Session {
            session_id: owner_session_id,
        },
        super::SourceAdmissionUse::SessionBodyInput,
        Some(super::SourceAdmissionOwnerRange::Session { start: 0, end: 2 }),
        None,
        2,
        Some(1),
    );
    // Replaying the same acceptance operation is idempotent. It must not
    // inflate the byte total merely because the actor emitted a duplicate
    // diagnostic callback.
    super::record_source_admission_dependency(
        &owner_ledger,
        Some(&source_context),
        super::SourceAdmissionOperationOwner::Session {
            session_id: owner_session_id,
        },
        super::SourceAdmissionUse::SessionBodyInput,
        Some(super::SourceAdmissionOwnerRange::Session { start: 0, end: 2 }),
        None,
        2,
        Some(1),
    );
    super::record_source_admission_dependency(
        &owner_ledger,
        Some(&source_context),
        super::SourceAdmissionOperationOwner::Session {
            session_id: owner_session_id,
        },
        super::SourceAdmissionUse::SessionBodyInput,
        Some(super::SourceAdmissionOwnerRange::Session { start: 2, end: 4 }),
        None,
        2,
        Some(2),
    );
    super::record_source_admission_dependency(
        &owner_ledger,
        Some(&source_context),
        super::SourceAdmissionOperationOwner::Session {
            session_id: owner_session_id,
        },
        super::SourceAdmissionUse::CandidateGateInput,
        Some(super::SourceAdmissionOwnerRange::Candidate { start: 0, end: 2 }),
        None,
        2,
        Some(1),
    );
    {
        let ledger = owner_ledger
            .lock()
            .expect("owner source admission ledger lock");
        assert_eq!(ledger.dependencies.len(), 2);
        let body = ledger
            .dependencies
            .iter()
            .find(|dependency| dependency.use_kind == super::SourceAdmissionUse::SessionBodyInput)
            .expect("body owner dependency");
        assert_eq!(body.accepted_bytes, 4);
        let body_snapshot = ledger
            .snapshots()
            .into_iter()
            .find(|snapshot| snapshot.use_kind == super::SourceAdmissionUse::SessionBodyInput)
            .expect("body owner dependency snapshot");
        assert_eq!(body_snapshot.owner_ranges.len(), 2);
        assert_eq!(
            body_snapshot.current_status,
            super::CaptureAdmissionBindingStatus::MatchedConsumed
        );
    }

    // A key collision with different observed bytes is a conflict, not a
    // second acceptance and not a silently ignored correction.
    let conflict_ledger = Arc::new(std::sync::Mutex::new(
        super::SourceAdmissionDependencyLedger::default(),
    ));
    super::record_source_admission_dependency(
        &conflict_ledger,
        Some(&source_context),
        super::SourceAdmissionOperationOwner::Session {
            session_id: owner_session_id,
        },
        super::SourceAdmissionUse::SessionBodyInput,
        Some(super::SourceAdmissionOwnerRange::Session { start: 0, end: 2 }),
        None,
        2,
        Some(10),
    );
    super::record_source_admission_dependency(
        &conflict_ledger,
        Some(&source_context),
        super::SourceAdmissionOperationOwner::Session {
            session_id: owner_session_id,
        },
        super::SourceAdmissionUse::SessionBodyInput,
        Some(super::SourceAdmissionOwnerRange::Session { start: 0, end: 2 }),
        None,
        3,
        Some(10),
    );
    {
        let ledger = conflict_ledger.lock().expect("conflict ledger");
        let body = ledger
            .dependencies
            .iter()
            .find(|dependency| dependency.use_kind == super::SourceAdmissionUse::SessionBodyInput)
            .expect("conflict body dependency");
        assert_eq!(body.accepted_bytes, 2);
        assert_eq!(body.operations.len(), 1);
        assert!(body.tracking_incomplete);
        assert!(ledger.is_incomplete());
    }

    // UNKNOWN aggregate bytes use the owner-scoped operation key: replaying
    // one operation is idempotent, distinct operations add, and an absent
    // operation id is never guessed to be a replay.
    let unknown_ledger = Arc::new(std::sync::Mutex::new(
        super::SourceAdmissionDependencyLedger::default(),
    ));
    let owner_a = new_session_id();
    let owner_b = new_session_id();
    for (bytes, operation_id, operation_owner) in [
        (
            2usize,
            Some(500u64),
            super::SourceAdmissionOperationOwner::Session {
                session_id: owner_a,
            },
        ),
        (
            2usize,
            Some(500u64),
            super::SourceAdmissionOperationOwner::Session {
                session_id: owner_a,
            },
        ),
        (
            2usize,
            Some(501u64),
            super::SourceAdmissionOperationOwner::Session {
                session_id: owner_a,
            },
        ),
        (
            3usize,
            Some(502u64),
            super::SourceAdmissionOperationOwner::Session {
                session_id: owner_b,
            },
        ),
        (
            2usize,
            None,
            super::SourceAdmissionOperationOwner::Session {
                session_id: owner_a,
            },
        ),
        (
            2usize,
            None,
            super::SourceAdmissionOperationOwner::Session {
                session_id: owner_a,
            },
        ),
    ] {
        super::record_source_admission_dependency(
            &unknown_ledger,
            None,
            operation_owner,
            super::SourceAdmissionUse::SessionBodyInputPossible,
            None,
            None,
            bytes,
            operation_id,
        );
    }
    let unknown_ledger = unknown_ledger.lock().expect("unknown operation ledger");
    let unknown = unknown_ledger
        .dependencies
        .iter()
        .find(|dependency| {
            dependency.operation_owner
                == super::SourceAdmissionOperationOwner::Session {
                    session_id: owner_a,
                }
        })
        .expect("unknown aggregate dependency");
    assert_eq!(unknown.accepted_bytes, 8);
    assert_eq!(unknown.operations.len(), 4);
    assert_eq!(
        unknown
            .operations
            .iter()
            .filter(|operation| operation.operation_id == Some(500))
            .count(),
        1
    );
    let unknown_b = unknown_ledger
        .dependencies
        .iter()
        .find(|dependency| {
            dependency.operation_owner
                == super::SourceAdmissionOperationOwner::Session {
                    session_id: owner_b,
                }
        })
        .expect("second owner unknown aggregate dependency");
    assert_eq!(unknown_b.accepted_bytes, 3);
    assert_eq!(unknown_b.operations.len(), 1);

    // The bounded projection window may rotate, but the active owner must
    // retain every distinct acceptance operation and its receipt reference.
    for index in 0..=4_096_u64 {
        super::record_source_admission_dependency(
            &owner_ledger,
            Some(&source_context),
            super::SourceAdmissionOperationOwner::Session {
                session_id: owner_session_id,
            },
            super::SourceAdmissionUse::SessionBodyInput,
            Some(super::SourceAdmissionOwnerRange::Session {
                start: 10 + index,
                end: 11 + index,
            }),
            None,
            1,
            Some(100 + index),
        );
    }
    {
        let ledger = owner_ledger
            .lock()
            .expect("owner source admission ledger lock after projection rotation");
        let body = ledger
            .dependencies
            .iter()
            .find(|dependency| dependency.use_kind == super::SourceAdmissionUse::SessionBodyInput)
            .expect("active body owner after projection rotation");
        assert_eq!(body.accepted_bytes, 4_101);
        assert_eq!(ledger.dependencies.len(), 2);
        assert_eq!(
            ledger.projections.len(),
            super::SOURCE_ADMISSION_DEPENDENCY_CAPACITY
        );
        let body_snapshot = ledger
            .snapshots()
            .into_iter()
            .find(|snapshot| snapshot.use_kind == super::SourceAdmissionUse::SessionBodyInput)
            .expect("body snapshot after projection rotation");
        assert!(body_snapshot.tracking_incomplete);
    }

    // A binding status is a historical snapshot. Re-reading the shared
    // witness after the bounded capture ledger evicts this receipt must still
    // expose the now-incomplete tracking state to any future qualification.
    for sequence in 1..=4_096_u16 {
        capture_collector
            .handle_notification(
                &crate::embedded_audio::build_audio_data_notification(910, sequence, &[3, 4])
                    .expect("capture ledger fill audio"),
            )
            .expect("capture ledger fill event");
    }
    assert!(capture_receipt.witness().metadata_incomplete);
    assert_eq!(
        super::capture_admission_binding_status(
            Some(910),
            Some(&capture_fact),
            Some(&capture_receipt),
            Some(&actor_fact),
            Some(actor_metadata),
            true,
        ),
        super::CaptureAdmissionBindingStatus::CaptureEvidenceIncomplete
    );
    let owner_snapshots = owner_ledger
        .lock()
        .expect("owner source admission ledger lock after eviction")
        .snapshots();
    let body_snapshot = owner_snapshots
        .iter()
        .find(|snapshot| snapshot.use_kind == super::SourceAdmissionUse::SessionBodyInput)
        .expect("body owner snapshot after eviction");
    assert_eq!(
        body_snapshot.current_status,
        super::CaptureAdmissionBindingStatus::CaptureEvidenceIncomplete
    );
}

#[test]
fn embedded_streaming_reset_prepares_background_listener_for_next_session() {
    let mut streaming = EmbeddedStreamingDictation::default();
    let pcm = pcm_from_samples(&[100, -100]);

    streaming
        .collector
        .handle_notification(&build_session_start_notification(7))
        .expect("start notification");
    streaming
        .collector
        .handle_notification(
            &build_audio_data_notification(7, 0, &pcm).expect("audio notification"),
        )
        .expect("audio notification");
    streaming
        .collector
        .handle_notification(&build_session_stop_notification(7, 1))
        .expect("stop notification");
    streaming.terminal_received = true;

    let result = streaming
        .submission_result()
        .expect("complete streaming result");
    assert_eq!(result.stats.session_id, Some(7));
    assert_eq!(result.stats.received_packet_count, 1);
    assert_eq!(result.reconstructed_pcm_bytes, pcm.len());

    streaming.reset_for_next_session();

    assert!(!streaming.terminal_received);
    assert!(streaming.session.is_none());
    assert!(streaming.embedded_session_id.is_none());
    assert!(streaming.pending_stop_expected_packet_count.is_none());
    assert_eq!(streaming.collector.inner().stats().session_id, None);
    assert!(streaming.submission_result().is_err());
}

#[test]
fn embedded_streaming_background_listener_keeps_pipeline_error_policy_after_reset() {
    let mut streaming = EmbeddedStreamingDictation::background_listener();

    assert!(streaming.keep_listening_after_pipeline_errors);
    streaming.terminal_received = true;
    streaming.embedded_session_id = Some(42);

    streaming.reset_for_next_session();

    assert!(streaming.keep_listening_after_pipeline_errors);
    assert!(!streaming.terminal_received);
    assert!(streaming.embedded_session_id.is_none());
    assert!(!EmbeddedStreamingDictation::default().keep_listening_after_pipeline_errors);
}

#[tokio::test]
async fn background_terminal_cancel_resets_without_fake_incomplete_submission() {
    let coordinator = Coordinator::new();
    let mut streaming = EmbeddedStreamingDictation::background_listener();
    streaming.embedded_session_id = Some(42);

    let completed = streaming
        .handle_ble_packet_actor_command(
            &coordinator.inner,
            StreamingSessionEvent::Cancelled {
                session_id: 42,
                expected_packet_count: 0,
            },
        )
        .await
        .expect("continuous cancel is handled without an error");

    assert!(
        !completed,
        "continuous cancel keeps the actor pending instead of requesting an empty submission"
    );
    assert!(streaming.embedded_session_id.is_none());
    assert!(streaming.session.is_none());
    assert!(streaming.submission_result().is_err());
}

#[tokio::test]
async fn background_terminal_error_is_recorded_once_without_empty_submission() {
    let coordinator = Coordinator::new();
    let mut streaming = EmbeddedStreamingDictation::background_listener();
    streaming.embedded_session_id = Some(43);

    let completed = streaming
        .handle_ble_packet_actor_command(
            &coordinator.inner,
            StreamingSessionEvent::Error {
                session_id: 43,
                expected_packet_count: 0,
                error_code: crate::embedded_audio::SessionErrorCode::Unknown(0xffff),
            },
        )
        .await
        .expect("continuous device error is contained without tearing down notify");

    assert!(
        !completed,
        "continuous error keeps the actor pending instead of requesting an empty submission"
    );
    assert!(streaming.embedded_session_id.is_none());
    assert!(streaming.session.is_none());
    assert!(streaming.submission_result().is_err());
}

#[test]
fn embedded_streaming_tail_chunk_remains_asr_input_until_the_session_drains() {
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    {
        let mut state = coordinator.inner.state.lock();
        state.session_id = session_id;
        state.phase = SessionPhase::Listening;
        state.cancelled = false;
    }
    let consumer = Arc::new(CapturingConsumer::default());
    let consumer_for_session: Arc<dyn crate::recorder::AudioConsumer> = consumer.clone();
    let mut session = embedded_audio_test_session(session_id, consumer_for_session);
    let before_stop = StreamingPcmChunk {
        session_id: 1,
        packet_sequence: 0,
        pcm: vec![1, 2],
        raw_input_level_percent: None,
        after_stop_boundary: false,
        metadata: None,
    };
    let after_stop = StreamingPcmChunk {
        session_id: 1,
        packet_sequence: 1,
        pcm: vec![3, 4],
        raw_input_level_percent: None,
        after_stop_boundary: true,
        metadata: None,
    };

    assert!(embedded_streaming_chunk_is_asr_input(&before_stop));
    assert!(embedded_streaming_chunk_is_asr_input(&after_stop));
    session
        .consume_streaming_pcm(
            &coordinator.inner,
            &after_stop.pcm,
            after_stop.raw_input_level_percent,
        )
        .expect("post-stop drain PCM is forwarded to ASR");
    session.flush_streaming_pcm();

    let chunks = consumer.chunks.lock().expect("capture lock");
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].len(), after_stop.pcm.len());
}

#[test]
fn embedded_ble_pcm_event_trace_is_sampled() {
    let first = StreamingSessionEvent::PcmChunk(StreamingPcmChunk {
        session_id: 1,
        packet_sequence: 0,
        pcm: vec![1, 2],
        raw_input_level_percent: None,
        after_stop_boundary: false,
        metadata: None,
    });
    let middle = StreamingSessionEvent::PcmChunk(StreamingPcmChunk {
        session_id: 1,
        packet_sequence: 17,
        pcm: vec![1, 2],
        raw_input_level_percent: None,
        after_stop_boundary: false,
        metadata: None,
    });
    let sample = StreamingSessionEvent::PcmChunk(StreamingPcmChunk {
        session_id: 1,
        packet_sequence: 50,
        pcm: vec![1, 2],
        raw_input_level_percent: None,
        after_stop_boundary: false,
        metadata: None,
    });
    let after_stop = StreamingSessionEvent::PcmChunk(StreamingPcmChunk {
        session_id: 1,
        packet_sequence: 51,
        pcm: vec![1, 2],
        raw_input_level_percent: None,
        after_stop_boundary: true,
        metadata: None,
    });

    assert!(embedded_ble_session_event_should_trace(&first));
    assert!(!embedded_ble_session_event_should_trace(&middle));
    assert!(embedded_ble_session_event_should_trace(&sample));
    assert!(embedded_ble_session_event_should_trace(&after_stop));
    assert!(embedded_ble_session_event_should_trace(
        &StreamingSessionEvent::Stopped {
            session_id: 1,
            expected_packet_count: 52,
            origin: crate::embedded_audio::SessionStopOrigin::User,
        }
    ));
}

#[test]
fn streamed_output_skips_postprocessing_mutations() {
    let rules = vec![correction_rule("Open AI", "OpenAI")];

    let result = finalize_polished_text(
        "Open AI".into(),
        false,
        false,
        PolishMode::Raw,
        &None,
        ChineseScriptPreference::Auto,
        &rules,
        true,
    );

    assert_eq!(result, "Open AI");
}

#[test]
fn raw_llm_output_still_applies_script_preference() {
    let result = finalize_polished_text(
        "繁體".into(),
        false,
        true,
        PolishMode::Raw,
        &None,
        ChineseScriptPreference::Simplified,
        &[],
        false,
    );

    assert_eq!(result, "繁体");
}

#[test]
fn non_streamed_output_still_applies_correction_rules() {
    let rules = vec![correction_rule("Open AI", "OpenAI")];

    let result = finalize_polished_text(
        "Open AI".into(),
        false,
        false,
        PolishMode::Raw,
        &None,
        ChineseScriptPreference::Auto,
        &rules,
        false,
    );

    assert_eq!(result, "OpenAI");
}

#[test]
fn append_typed_prefix_keeps_unicode_char_boundaries() {
    let mut typed = String::from("前");

    let appended = append_typed_prefix(&mut typed, "a你🙂b", 3);

    assert_eq!(appended, 3);
    assert_eq!(typed, "前a你🙂");
}

#[test]
fn append_typed_prefix_caps_at_delta_length() {
    let mut typed = String::new();

    let appended = append_typed_prefix(&mut typed, "好", 10);

    assert_eq!(appended, 1);
    assert_eq!(typed, "好");
}

#[test]
fn wayland_disables_streaming_insert_even_when_pref_enabled() {
    assert!(!streaming_insert_eligible(
        true,
        false,
        PolishMode::Light,
        false,
        true
    ));
}

#[test]
fn x11_linux_can_still_use_streaming_insert_when_other_gates_pass() {
    assert!(streaming_insert_eligible(
        true,
        false,
        PolishMode::Light,
        false,
        false
    ));
}

#[test]
fn raw_mode_without_llm_uses_passthrough_instead_of_streaming_polish() {
    assert!(!streaming_insert_eligible(
        true,
        false,
        PolishMode::Raw,
        false,
        false
    ));
    assert!(streaming_insert_eligible(
        true,
        false,
        PolishMode::Raw,
        true,
        false
    ));
}

#[test]
fn wayland_done_message_tells_user_manual_paste_is_required() {
    assert_eq!(
        wayland_done_message(InsertStatus::CopiedFallback, false),
        Some("Wayland 未启用自动输入，已复制到剪贴板，请手动粘贴".to_string())
    );
    assert_eq!(
        wayland_done_message(InsertStatus::CopiedFallback, true),
        Some("Wayland 未启用自动输入，已复制原文到剪贴板，请手动粘贴".to_string())
    );
    assert_eq!(
        wayland_done_message(InsertStatus::Failed, false),
        Some("Wayland 未启用自动输入，剪贴板写入失败".to_string())
    );
}

#[test]
fn automatic_speaker_candidate_reaches_asr_only_after_verified_match() {
    assert!(super::speaker_candidate_may_reach_asr(false, None));
    assert!(super::speaker_candidate_may_reach_asr(true, Some(true)));
    assert!(!super::speaker_candidate_may_reach_asr(true, Some(false)));
    assert!(!super::speaker_candidate_may_reach_asr(true, None));
}

#[test]
fn phrase_hit_waits_for_real_owner_audio_without_requiring_a_pause() {
    // No voiceprint enrolled for the configured phrase: phrase hit is enough.
    // The 1.1s owner window only applies to a phrase-bound owner template.
    assert!(!super::owner_verification_window_ready(
        super::OWNER_VERIFICATION_START_BYTES - 2,
        true,
    ));
    assert!(super::owner_verification_window_ready(
        super::OWNER_VERIFICATION_START_BYTES,
        true,
    ));
    assert!(super::owner_verification_window_ready(0, false));
    assert!(super::owner_verification_window_ready(
        super::OWNER_VERIFICATION_START_BYTES - 2,
        false,
    ));
    assert_eq!(super::next_owner_verification_retry_ms(1_100), Some(1_800));
    assert_eq!(super::next_owner_verification_retry_ms(1_800), Some(2_400));
    assert_eq!(super::next_owner_verification_retry_ms(2_400), None);
}

#[test]
fn short_or_failed_early_owner_window_retries_before_fail_closed_reject() {
    let short = Err("voiceprint audio is shorter than 1000 ms".to_string());
    assert_eq!(
        super::next_owner_verification_retry_after(1_111, &short),
        Some(1_800)
    );
    assert_eq!(
        super::next_owner_verification_retry_after(1_800, &short),
        Some(2_400)
    );
    assert_eq!(
        super::next_owner_verification_retry_after(1_869, &short),
        Some(1_869)
    );
    assert_eq!(
        super::next_owner_verification_retry_after(2_400, &short),
        None,
        "verification errors remain fail-closed after the bounded retry ladder"
    );

    let matched = Ok(crate::speaker_verification::VerificationResult {
        matched: true,
        owner_matched: true,
        score: 0.58,
        policy: crate::speaker_verification::VerificationPolicy::Enrolled,
    });
    assert_eq!(
        super::next_owner_verification_retry_after(1_111, &matched),
        None
    );
}

#[test]
fn owner_verification_prefetch_requires_enrollment_and_a_real_model_window() {
    assert!(!super::should_prefetch_owner_verification(
        false,
        false,
        super::OWNER_VERIFICATION_START_BYTES,
    ));
    assert!(!super::should_prefetch_owner_verification(
        true,
        true,
        super::OWNER_VERIFICATION_START_BYTES,
    ));
    assert!(!super::should_prefetch_owner_verification(
        true,
        false,
        super::OWNER_VERIFICATION_START_BYTES - 2,
    ));
    assert!(super::should_prefetch_owner_verification(
        true,
        false,
        super::OWNER_VERIFICATION_START_BYTES,
    ));
}

#[test]
fn ambiguous_owner_requires_two_consistent_phrase_backed_snapshots() {
    let mut confirmations = 0;
    let mut best_score = 0.0;
    assert!(!super::note_ambiguous_owner_evidence(
        &mut confirmations,
        &mut best_score,
        denzic_voice_activation_v1_core::PhraseSignal::KeywordModel,
        0.3847,
    ));
    assert_eq!(confirmations, 1);
    assert!(super::note_ambiguous_owner_evidence(
        &mut confirmations,
        &mut best_score,
        denzic_voice_activation_v1_core::PhraseSignal::KeywordModel,
        0.3998,
    ));
    assert_eq!(confirmations, 2);
    assert!((best_score - 0.3998).abs() < f32::EPSILON);
}

#[test]
fn ambiguous_owner_recovery_decays_on_a_weak_window_and_resets_without_phrase() {
    let mut confirmations = 1;
    let mut best_score = 0.40;
    assert!(!super::note_ambiguous_owner_evidence(
        &mut confirmations,
        &mut best_score,
        denzic_voice_activation_v1_core::PhraseSignal::KeywordModel,
        0.33,
    ));
    assert_eq!(confirmations, 0);
    assert!((best_score - 0.40).abs() < f32::EPSILON);

    assert!(!super::note_ambiguous_owner_evidence(
        &mut confirmations,
        &mut best_score,
        denzic_voice_activation_v1_core::PhraseSignal::None,
        0.60,
    ));
    assert_eq!(confirmations, 0);
    assert_eq!(best_score, 0.0);
}

#[test]
fn ambiguous_owner_history_survives_one_noisy_window() {
    let mut confirmations = 0;
    let mut best_score = 0.0;
    assert!(!super::note_ambiguous_owner_evidence(
        &mut confirmations,
        &mut best_score,
        denzic_voice_activation_v1_core::PhraseSignal::KeywordModel,
        0.40,
    ));
    assert!(super::note_ambiguous_owner_evidence(
        &mut confirmations,
        &mut best_score,
        denzic_voice_activation_v1_core::PhraseSignal::KeywordModel,
        0.40,
    ));
    assert!(!super::note_ambiguous_owner_evidence(
        &mut confirmations,
        &mut best_score,
        denzic_voice_activation_v1_core::PhraseSignal::KeywordModel,
        0.30,
    ));
    assert_eq!(confirmations, 1);
    assert!(super::note_ambiguous_owner_evidence(
        &mut confirmations,
        &mut best_score,
        denzic_voice_activation_v1_core::PhraseSignal::KeywordModel,
        0.40,
    ));
    assert_eq!(confirmations, 2);
}

#[test]
fn complete_local_phrase_recovers_noisy_owner_but_never_kws_or_errors() {
    let noisy_owner = Ok(crate::speaker_verification::VerificationResult {
        matched: false,
        owner_matched: false,
        score: 0.26448274,
        policy: crate::speaker_verification::VerificationPolicy::Enrolled,
    });
    assert!(!super::local_phrase_can_recover_owner_gate(
        denzic_voice_activation_v1_core::PhraseSignal::LocalTranscript,
        1_799,
        &noisy_owner,
    ));
    assert!(super::local_phrase_can_recover_owner_gate(
        denzic_voice_activation_v1_core::PhraseSignal::LocalTranscript,
        1_809,
        &noisy_owner,
    ));
    assert!(!super::local_phrase_can_recover_owner_gate(
        denzic_voice_activation_v1_core::PhraseSignal::KeywordModel,
        2_400,
        &noisy_owner,
    ));

    let explicit_non_owner = Ok(crate::speaker_verification::VerificationResult {
        matched: false,
        owner_matched: false,
        score: 0.153456,
        policy: crate::speaker_verification::VerificationPolicy::Enrolled,
    });
    assert!(!super::local_phrase_can_recover_owner_gate(
        denzic_voice_activation_v1_core::PhraseSignal::LocalTranscript,
        2_400,
        &explicit_non_owner,
    ));
    assert!(!super::local_phrase_can_recover_owner_gate(
        denzic_voice_activation_v1_core::PhraseSignal::LocalTranscript,
        2_400,
        &Err("voiceprint runtime failed".to_string()),
    ));
}

#[test]
fn kws_owner_fallback_recovers_field_score_but_keeps_non_owner_floor() {
    let field_owner = Ok(crate::speaker_verification::VerificationResult {
        matched: false,
        owner_matched: false,
        score: 0.330_721,
        policy: crate::speaker_verification::VerificationPolicy::Enrolled,
    });
    assert!(super::kws_can_recover_owner_gate(
        denzic_voice_activation_v1_core::PhraseSignal::KeywordModel,
        2_529,
        &field_owner,
    ));

    let explicit_non_owner = Ok(crate::speaker_verification::VerificationResult {
        matched: false,
        owner_matched: false,
        score: 0.20,
        policy: crate::speaker_verification::VerificationPolicy::Enrolled,
    });
    assert!(!super::kws_can_recover_owner_gate(
        denzic_voice_activation_v1_core::PhraseSignal::KeywordModel,
        2_529,
        &explicit_non_owner,
    ));
    assert!(!super::kws_can_recover_owner_gate(
        denzic_voice_activation_v1_core::PhraseSignal::LocalTranscript,
        2_529,
        &field_owner,
    ));
}

#[test]
fn complete_local_phrase_fast_accepts_near_threshold_owner_only() {
    // Installed session 212: the phrase was ExactStart and the freshly enrolled
    // owner scored 0.402310. It must not wait for a second owner snapshot.
    let near_owner = Ok(crate::speaker_verification::VerificationResult {
        matched: false,
        owner_matched: false,
        score: 0.402_310,
        policy: crate::speaker_verification::VerificationPolicy::Enrolled,
    });
    assert!(super::local_phrase_can_fast_accept_owner_gate(
        denzic_voice_activation_v1_core::PhraseSignal::LocalTranscript,
        &near_owner,
    ));
    assert!(!super::local_phrase_can_fast_accept_owner_gate(
        denzic_voice_activation_v1_core::PhraseSignal::KeywordModel,
        &near_owner,
    ));

    let low_non_owner = Ok(crate::speaker_verification::VerificationResult {
        matched: false,
        owner_matched: false,
        score: 0.399_999,
        policy: crate::speaker_verification::VerificationPolicy::Enrolled,
    });
    assert!(!super::local_phrase_can_fast_accept_owner_gate(
        denzic_voice_activation_v1_core::PhraseSignal::LocalTranscript,
        &low_non_owner,
    ));
    assert!(!super::local_phrase_can_fast_accept_owner_gate(
        denzic_voice_activation_v1_core::PhraseSignal::LocalTranscript,
        &Err("voiceprint runtime failed".to_string()),
    ));
}

#[test]
fn installed_terminal_session_210_uses_fused_owner_recovery() {
    // Live terminal candidate 210: both phrase stages found an exact
    // "开始录音", but the enrolled voiceprint varied to 0.343854 and the
    // terminal call site used to bypass this shared recovery policy.
    let verification = Ok(crate::speaker_verification::VerificationResult {
        matched: false,
        owner_matched: false,
        score: 0.343_854,
        policy: crate::speaker_verification::VerificationPolicy::Enrolled,
    });
    let mut confirmations = 0;
    let mut best_score = 0.0;
    let owner_gate = super::evaluate_owner_gate_evidence(
        &mut confirmations,
        &mut best_score,
        denzic_voice_activation_v1_core::PhraseSignal::LocalTranscript,
        4_660,
        &verification,
    );
    assert_eq!(
        owner_gate.access,
        crate::speech_decision_kernel::OwnerAccessEvidence::EnrolledMatch
    );
    assert!(owner_gate.recovered_by_local_phrase);

    let kws_only = super::evaluate_owner_gate_evidence(
        &mut confirmations,
        &mut best_score,
        denzic_voice_activation_v1_core::PhraseSignal::KeywordModel,
        4_660,
        &verification,
    );
    assert_eq!(
        kws_only.access,
        crate::speech_decision_kernel::OwnerAccessEvidence::EnrolledMatch
    );
    assert!(kws_only.recovered_by_local_phrase);

    let stream = include_str!("dictation_embedded_stream.rs");
    let terminal_gate = stream
        .find("Installed sessions 210/212")
        .expect("terminal regression annotation must remain");
    assert!(
        stream[terminal_gate..].contains("arbitrate_candidate_wake("),
        "terminal gate must use the same fused owner policy as the live path"
    );
}

#[test]
fn target_speaker_endpoint_wake_terminal_and_live_paths_share_one_arbitration_seam() {
    let stream = include_str!("dictation_embedded_stream.rs");
    let uses = stream.matches("arbitrate_candidate_wake(").count();
    assert_eq!(
        uses, 2,
        "terminal and live call sites must remain on the single arbitration seam"
    );
    assert_eq!(
        stream
            .matches("speech_decision_kernel::arbitrate_wake(")
            .count(),
        0,
        "streaming coordinator must not bypass the shared arbitration seam"
    );
}

#[test]
fn local_confirmation_adds_context_with_a_strict_attempt_cap() {
    assert_eq!(
        super::next_local_confirmation_snapshot_bytes(0),
        Some(800 * 32)
    );
    assert_eq!(
        super::next_local_confirmation_snapshot_bytes(1),
        Some(1_800 * 32)
    );
    assert_eq!(
        super::next_local_confirmation_snapshot_bytes(2),
        Some(2_000 * 32)
    );
    assert_eq!(
        super::next_local_confirmation_snapshot_bytes(3),
        Some(2_400 * 32)
    );
    assert_eq!(
        super::next_local_confirmation_snapshot_bytes(4),
        Some(3_000 * 32)
    );
    assert_eq!(
        super::next_local_confirmation_snapshot_bytes(5),
        Some(5_000 * 32)
    );
    assert_eq!(super::next_local_confirmation_snapshot_bytes(6), None);
}

#[test]
fn local_confirmation_ladder_does_not_block_the_complete_phrase_window() {
    // Installed 569/578/583: the 1.4 s inference was still running when the
    // useful 1.8 s phrase tail arrived. The helper is single-flight, so that
    // rung increased latency instead of recall.
    let snapshots = (0..6)
        .map(|attempt| super::next_local_confirmation_snapshot_bytes(attempt).unwrap() / 32)
        .collect::<Vec<_>>();
    assert_eq!(snapshots, vec![800, 1_800, 2_000, 2_400, 3_000, 5_000]);
    assert!(!snapshots.contains(&1_400));
    assert!(!snapshots.contains(&1_600));
}

/// 2026-09-22 跟手③:音近证据加密重试的状态机——武装/节奏/预算三道门。
#[cfg(target_os = "windows")]
#[test]
fn near_retry_dense_cadence_requires_arm_new_audio_and_budget() {
    let mut state = super::LocalNearRetryState::default();
    // 未武装(安静无证据)不跟拍——梯子节奏原样。
    assert!(!state.should_fire(super::LOCAL_NEAR_RETRY_NEW_AUDIO_BYTES));
    state.pending = true;
    // 武装后也要等够 ~250ms 新音频。
    assert!(!state.should_fire(super::LOCAL_NEAR_RETRY_NEW_AUDIO_BYTES - 1));
    assert!(state.should_fire(super::LOCAL_NEAR_RETRY_NEW_AUDIO_BYTES));
    // 触发即消耗武装、计数、标记在飞(完成侧读后清除)。
    state.note_fired();
    assert!(!state.pending);
    assert!(state.in_flight);
    assert_eq!(state.fires, 1);
    // 预算耗尽后即使重新武装也不再跟。
    state.pending = true;
    state.fires = super::LOCAL_NEAR_RETRY_MAX_FIRES;
    assert!(!state.should_fire(super::LOCAL_NEAR_RETRY_NEW_AUDIO_BYTES));
}

/// 音近加密重试只认"近满长+距离≤1"的证据:半截短语/无关键内容不武装。
#[cfg(target_os = "windows")]
#[test]
fn near_retry_arms_only_on_full_length_near_evidence() {
    let near = super::LocalWakeConfirmation {
        matched: false,
        phrase_relation: crate::wake_phrase::LocalPhraseRelation::Absent,
        transcript_chars: 4,
        phonetic_prefix_units: 3,
        phonetic_suffix_units: 0,
        phonetic_best_distance: 1,
        phonetic_best_window_start: 0,
        inference_ms: 120,
        snapshot_pcm_ms: 1_600,
        recovered_keyword_end_seconds: None,
    };
    assert!(super::local_near_retry_should_arm(&near, 5, 0));

    // 半截短语(长度不足)不武装——0.8s 窗口只听到"开始"时保持梯子节奏。
    let partial = super::LocalWakeConfirmation {
        transcript_chars: 2,
        ..near
    };
    assert!(!super::local_near_retry_should_arm(&partial, 5, 0));

    // 距离太远(≥2)不武装。
    let far = super::LocalWakeConfirmation {
        phonetic_best_distance: 2,
        ..near
    };
    assert!(!super::local_near_retry_should_arm(&far, 5, 0));

    // 已匹配的不武装(接受路径自会处理)。
    let matched = super::LocalWakeConfirmation {
        matched: true,
        ..near
    };
    assert!(!super::local_near_retry_should_arm(&matched, 5, 0));

    // 预算耗尽不武装。
    assert!(!super::local_near_retry_should_arm(
        &near,
        5,
        super::LOCAL_NEAR_RETRY_MAX_FIRES
    ));
}

#[cfg(target_os = "windows")]
#[test]
fn target_speaker_endpoint_strong_start_prefix_gets_one_non_authoritative_latency_followup() {
    let partial = super::LocalWakeConfirmation {
        matched: false,
        phrase_relation: crate::wake_phrase::LocalPhraseRelation::Absent,
        transcript_chars: 2,
        phonetic_prefix_units: 2,
        phonetic_suffix_units: 0,
        phonetic_best_distance: 2,
        phonetic_best_window_start: 0,
        inference_ms: 150,
        snapshot_pcm_ms: 1_600,
        recovered_keyword_end_seconds: None,
    };
    assert!(super::local_confirmation_prefix_retry_eligible(&partial, 4));

    let unrelated = super::LocalWakeConfirmation {
        phonetic_prefix_units: 0,
        phonetic_suffix_units: 0,
        phonetic_best_distance: 4,
        ..partial
    };
    assert!(!super::local_confirmation_prefix_retry_eligible(
        &unrelated, 4
    ));

    let later_window = super::LocalWakeConfirmation {
        phonetic_best_window_start: 1,
        ..partial
    };
    assert!(!super::local_confirmation_prefix_retry_eligible(
        &later_window,
        4
    ));

    assert_eq!(super::LOCAL_CONFIRMATION_PREFIX_RETRY_NEW_AUDIO_MS, 140);
    assert_eq!(super::LOCAL_CONFIRMATION_PREFIX_RETRY_AFTER_ATTEMPTS, 1);

    let mut rolling = super::LocalConfirmationPrefixRetryState {
        pending: true,
        retry_after_attempts: 1,
        ..Default::default()
    };
    assert!(rolling.should_start(
        false,
        1,
        super::LOCAL_CONFIRMATION_PREFIX_RETRY_NEW_AUDIO_BYTES,
    ));
    assert!(rolling.blocks_heavy_recovery(false));
    rolling.note_started(true);
    assert!(rolling.blocks_heavy_recovery(true));
    assert!(!rolling.blocks_heavy_recovery(false));
}

#[cfg(target_os = "windows")]
#[test]
fn bounded_rolling_owner_near_match_reaches_voiceprint_gate_without_swallowing_body() {
    let session_169_followup = super::LocalWakeConfirmation {
        matched: false,
        phrase_relation: crate::wake_phrase::LocalPhraseRelation::Absent,
        transcript_chars: 7,
        phonetic_prefix_units: 3,
        phonetic_suffix_units: 0,
        phonetic_best_distance: 1,
        phonetic_best_window_start: 0,
        inference_ms: 314,
        snapshot_pcm_ms: 1_880,
        recovered_keyword_end_seconds: None,
    };
    assert!(super::live_owner_near_wake_can_attempt(
        true,
        true,
        &session_169_followup,
        4,
        1_004 * 32,
        1_004 * 32,
    ));
    assert!(!super::live_owner_near_wake_can_attempt(
        false,
        true,
        &session_169_followup,
        4,
        1_004 * 32,
        1_004 * 32,
    ));
    assert!(!super::live_owner_near_wake_can_attempt(
        true,
        false,
        &session_169_followup,
        4,
        1_004 * 32,
        1_004 * 32,
    ));
    let wake_end = super::live_owner_near_wake_end_seconds(&session_169_followup, 4, 1_004 * 32);
    assert!((wake_end - 2.078).abs() < 0.002);
    assert!(wake_end < 1.004 + session_169_followup.snapshot_pcm_ms as f32 / 1_000.0);
}

#[test]
fn device_key_start_takeover_pending_before_hidden_active() {
    let polish = include_str!("dictation_wake_polish.rs");
    let kernel = include_str!("../speech_decision_kernel.rs");
    assert!(
        polish.contains("note_device_key_dictation_start_intent")
            && kernel.contains("device_key_takeover_pending")
            && kernel.contains("candidate_promotion_requested"),
        "device-key Start must sticky-promote when the hidden VA candidate is not ACTIVE yet"
    );
    let hotkey = include_str!("hotkey_device_runtime.rs");
    assert!(
        hotkey.contains("note_device_key_dictation_start_intent(&inner)"),
        "device-key dictation Start must call note_device_key_dictation_start_intent"
    );
    // Promote stays internal (ACTIVATE vs TOGGLE). Capsule copy must not say "接管"
    // — owners treat that as a product bug when voice-auto-start only had a hidden buffer.
    assert!(
        !hotkey.contains("\"正在接管当前录音...\""),
        "device-key Start capsule must not emit the legacy taking-over status string"
    );
    assert!(
        hotkey.contains("\"正在启动 Listener 录音...\""),
        "device-key Start capsule must use normal start copy even when promoting hidden VA"
    );
}

#[test]
fn wake_candidate_controller_has_no_legacy_split_state() {
    let polish = include_str!("dictation_wake_polish.rs");
    let dictation = include_str!("dictation.rs");
    let kernel = include_str!("../speech_decision_kernel.rs");
    assert!(!polish.contains("WakeCandidateController"));
    assert!(!polish.contains("HIDDEN_AUTOMATIC_CANDIDATE_"));
    assert!(kernel.contains("struct RecordingLifecycleController"));
    assert!(kernel.contains("fn take_candidate_promotion"));
    assert!(!dictation.contains("LAST_HIDDEN_VA_SESSION"));
}

#[test]
fn hidden_candidate_marked_active_before_detector_init() {
    // Device-key promote depends on ACTIVE during StreamingDetector::new (~2s).
    let stream = concat!(
        include_str!("dictation_embedded_stream.rs"),
        "\n",
        include_str!("dictation_embedded_candidate_begin.rs"),
        "\n",
        include_str!("dictation_embedded_detector_init.rs")
    );
    let begin = stream
        .find("async fn begin_candidate_or_session")
        .expect("begin_candidate_or_session");
    let body = &stream[begin..];
    let mark = body
        .find(".begin_candidate(embedded_session_id)")
        .expect("must bind the hidden candidate identity for Verification");
    let detector = body
        .find("StreamingDetector::new(&phrase)")
        .expect("detector init");
    assert!(
        mark < detector,
        "RecordingLifecycleController::begin_candidate must run before StreamingDetector::new so EC11 Start can promote instead of toggle-stop"
    );
    assert!(
        body.contains("detector_deferred") && body.contains("wake_detector_init"),
        "detector init must be deferred so PCM buffers during StreamingDetector::new"
    );
    let stream_all = stream;
    let dictation = include_str!("dictation.rs");
    assert!(
        stream_all.contains("show_early_wake_recording_capsule")
            && dictation.contains("local full-phrase confirmed")
            && stream_all.contains("PendingSecondaryDecision::AwaitSecondary")
            && !stream_all.contains("stage2 timeout fail-open KeywordModel")
            && stream_all.contains("stage2 timeout held after explicit Absent")
            && stream_all.contains("terminal stage2 unavailable held after explicit Absent")
            && stream_all.contains("terminal stage2 task failure held after explicit Absent")
            && stream_all.contains("stage2 Absent reject")
            && stream_all.contains("KWS_SECONDARY_CONFIRM_BUDGET_MS"),
        "stage2 waits for evidence; explicit Absent remains authoritative through terminal confirmation"
    );
}

#[test]
fn orphan_pcm_without_explicit_start_must_not_open_dictation() {
    // Type restart / notify reopen can receive mid-stream PCM (no SessionStart).
    // That must not open a Recording capsule (phantom dictation).
    let stream = include_str!("dictation_embedded_stream.rs");
    assert!(
        stream.contains("ignoring orphan embedded PCM without explicit start")
            && stream.contains("if self.session.is_none()")
            && stream.contains("no phantom recording on reconnect"),
        "PcmChunk path must drop orphan audio until explicit SessionStart creates session/candidate"
    );
    // The guard must sit before begin_session_if_needed on the no-candidate branch.
    let pcm_arm = stream
        .find("StreamingSessionEvent::PcmChunk(chunk)")
        .expect("pcm arm");
    let orphan = stream[pcm_arm..]
        .find("ignoring orphan embedded PCM without explicit start")
        .expect("orphan guard")
        + pcm_arm;
    let begin = stream[pcm_arm..]
        .find("self.begin_session_if_needed(inner, chunk.session_id)")
        .expect("begin_session_if_needed on pcm path")
        + pcm_arm;
    assert!(
        orphan < begin,
        "orphan PCM guard must run before begin_session_if_needed"
    );
}

#[test]
fn orphan_recovery_failure_is_quarantined_per_embedded_session() {
    let stream = include_str!("dictation_embedded_stream.rs");
    let polish = include_str!("dictation_wake_polish.rs");
    assert!(
        polish.contains("orphan_recovery_quarantine_session_id"),
        "stream actor must retain a per-session orphan recovery tombstone"
    );
    assert!(
        stream.contains("dropping quarantined orphan embedded PCM")
            && stream.contains("orphan embedded PCM recovery rejected once; quarantining session")
            && stream.contains("coalescing quarantined embedded SessionStart"),
        "failed orphan recovery must be coalesced instead of restarting the actor per packet"
    );
}

#[test]
fn automatic_start_never_bypasses_hidden_candidate_gate() {
    use crate::embedded_audio::SessionStartOrigin;

    assert_eq!(
        super::buffered_speaker_candidate_kind(SessionStartOrigin::User, false, false),
        None
    );
    assert_eq!(
        super::buffered_speaker_candidate_kind(SessionStartOrigin::VoiceActivation, false, true),
        Some(super::BufferedSpeakerCandidateKind::Verification)
    );
    // Deleting the voiceprint must not disable automatic wake: still enter the
    // verification gate path; speaker_verification::verify open-gates when empty.
    assert_eq!(
        super::buffered_speaker_candidate_kind(SessionStartOrigin::VoiceActivation, false, false),
        Some(super::BufferedSpeakerCandidateKind::Verification)
    );
    assert_eq!(
        super::buffered_speaker_candidate_kind(SessionStartOrigin::Unknown(9), false, true),
        Some(super::BufferedSpeakerCandidateKind::Rejected)
    );

    let source = concat!(
        include_str!("dictation.rs"),
        "
",
        include_str!("dictation_preview.rs"),
        "
",
        include_str!("dictation_device_ai.rs"),
        "
",
        include_str!("dictation_wake_polish.rs"),
        "
",
        include_str!("dictation_session.rs"),
        "
",
        include_str!("dictation_embedded_submit.rs"),
        "
",
        include_str!("dictation_embedded_stream.rs"),
        "
",
        include_str!("dictation_embedded_candidate_begin.rs"),
        "
",
        include_str!("dictation_embedded_detector_init.rs"),
        "
",
        include_str!("dictation_embedded_stream_completion.rs")
    );
    assert!(
        source.contains("crate::speaker_verification::is_enrolled_for_phrase(&phrase)")
            && source.contains("fn owner_verification_window_ready"),
        "no-voiceprint path must skip the owner speech window delay for the configured phrase"
    );
    // Hidden ACTIVE must be marked before StreamingDetector::new (~1–2s init)
    // so device-key Start promotes instead of toggle-stop during that window.
    let mark_hidden = source
        .find(".begin_candidate(embedded_session_id)")
        .expect("hidden automatic candidate identity must be bound");
    let wake_init = source[mark_hidden..]
        .find("let wake_detector_init =")
        .map(|offset| mark_hidden + offset)
        .expect("wake detector init after hidden ACTIVE mark");
    let wake_window = &source[wake_init..wake_init + 1200.min(source.len() - wake_init)];
    assert!(
        wake_window.contains("StreamingDetector::new(&phrase)")
            && !wake_window.contains("StreamingDetector::new_strict"),
        "primary automatic wake must use StreamingDetector::new (sensitive), not new_strict"
    );
    let start = source
        .find("async fn try_release_automatic_candidate")
        .expect("live automatic gate should exist");
    let end = source[start..]
        .find("async fn finish_completed_streaming_session")
        .map(|offset| start + offset)
        .expect("live automatic gate boundary");
    let body = &source[start..end];
    assert!(body.contains("detector.accept_pcm(&pcm_to_feed)"));
    assert!(body.contains("rolling_kws_rotation_start("));
    assert!(body.contains("STREAMING_KWS_ROTATE_OVERLAP_MS"));
    assert!(body.contains("candidate.kws_fed_bytes = candidate.pcm.len()"));
    assert!(body.contains("crate::speaker_verification::verify(&pcm, &voiceprint_phrase)"));
    assert!(
        body.find("detector.accept_pcm(&pcm_to_feed)")
            < body.find("crate::speaker_verification::verify(&pcm, &voiceprint_phrase)")
    );
    assert!(
        body.find("let recording_control_task")
            < body.find("begin_embedded_audio_dictation_session")
    );
    assert!(
        body.find("session.consume_streaming_pcm") < body.find("let _recording_control_observer")
    );
    assert!(
        body.find("let _recording_control_observer") < body.find("recording_control_task.await")
    );
    assert!(
        body.contains("tauri::async_runtime::spawn(async move"),
        "automatic activation completion must be observed outside the BLE actor"
    );
    assert!(
        !body.contains("let recording_control_ms = match recording_control_task.await"),
        "the BLE notification actor must never await its own active-control queue"
    );
    assert!(body.contains("recording_control=detached"));
    assert!(body.contains("early_capsule_request_ms"));
    assert!(body.contains("latency_target_ms=1200"));
    assert!(body.contains("latency_ceiling_ms=1500"));
}

#[test]
fn automatic_activation_does_not_await_its_own_ble_actor_queue() {
    let source = include_str!("dictation_embedded_stream.rs");
    let start = source
        .find("async fn try_release_automatic_candidate")
        .expect("live automatic gate should exist");
    let body = &source[start..];

    let actor_pcm = body
        .find("session.consume_streaming_pcm")
        .expect("accepted candidate PCM must enter the formal session");
    let detached_observer = body
        .find("let _recording_control_observer")
        .expect("activation result must have a detached observer");
    let control_await = body
        .find("recording_control_task.await")
        .expect("detached observer must retain activation result logging");

    assert!(actor_pcm < detached_observer);
    assert!(detached_observer < control_await);
    assert!(body.contains("tauri::async_runtime::spawn(async move"));
    assert!(body.contains("recording_control=detached"));
    assert!(!body.contains("let recording_control_ms = match recording_control_task.await"));
}

#[test]
fn physical_hidden_candidate_promotion_discards_pre_press_pcm() {
    let mut pcm = vec![1, 2, 3, 4, 5, 6];
    assert_eq!(super::discard_pre_press_candidate_pcm(&mut pcm), 6);
    assert!(pcm.is_empty());

    let source = concat!(
        include_str!("dictation.rs"),
        "
",
        include_str!("dictation_preview.rs"),
        "
",
        include_str!("dictation_device_ai.rs"),
        "
",
        include_str!("dictation_wake_polish.rs"),
        "
",
        include_str!("dictation_session.rs"),
        "
",
        include_str!("dictation_embedded_submit.rs"),
        "
",
        include_str!("dictation_embedded_stream.rs")
    );
    let start = source
        .find("async fn promote_hidden_candidate_if_requested")
        .expect("physical hidden-candidate promotion should exist");
    let end = source[start..]
        .find("async fn try_release_automatic_candidate")
        .map(|offset| start + offset)
        .expect("physical promotion helper boundary");
    let body = &source[start..end];
    assert!(body.contains("discard_pre_press_candidate_pcm(&mut candidate.pcm)"));
    assert!(!body.contains("session.consume_streaming_pcm"));
    // 候选必须先到再消费 promotion，否则第一次物理键按下会把 promotion 吃掉却开不了录音。
    let candidate_take = body
        .find("self.speaker_candidate.take()")
        .expect("promotion must take speaker candidate first");
    let promotion_take = body
        .find("take_hidden_automatic_candidate_promotion(inner, embedded_session_id)")
        .expect("promotion must consume the promotion flag");
    assert!(
        candidate_take < promotion_take,
        "candidate must arrive before promotion is consumed"
    );
}

#[test]
fn device_processing_completion_requires_a_matching_start() {
    assert!(!super::device_ai_processing_completion_allowed(
        false, false
    ));
    assert!(super::device_ai_processing_completion_allowed(true, false));
    assert!(!super::device_ai_processing_completion_allowed(true, true));
}

#[test]
fn hidden_candidate_rejection_has_no_processing_led_command() {
    let source = concat!(
        include_str!("dictation.rs"),
        "
",
        include_str!("dictation_preview.rs"),
        "
",
        include_str!("dictation_device_ai.rs"),
        "
",
        include_str!("dictation_wake_polish.rs"),
        "
",
        include_str!("dictation_session.rs"),
        "
",
        include_str!("dictation_embedded_submit.rs"),
        "
",
        include_str!("dictation_embedded_stream.rs")
    );
    let start = source
        .find("fn reject_hidden_automatic_candidate")
        .expect("hidden rejection helper should exist");
    let end = source[start..]
        .find("fn complete_voiceprint_enrollment_candidate")
        .map(|offset| start + offset)
        .expect("voiceprint enrollment helper should follow rejection helper");
    let body = &source[start..end];

    assert!(!body.contains("send_recording_processing_"));
    assert!(body.contains("rejected silently"));

    let start = source
        .find("async fn finish_end_session_after_stop_transition")
        .expect("stop pipeline should exist");
    let end = source[start..]
        .find("pub(super) fn dictation_error_code")
        .map(|offset| start + offset)
        .expect("stop pipeline boundary should exist");
    let body = &source[start..end];
    let processing_start = body
        .find("dictation_transcribing_processing_start")
        .expect("processing LED should start with the transcribing phase");
    let final_result_wait = body
        .find("asr.await_final_result_with_early_seal()")
        .or_else(|| body.find("asr.await_final_result()"))
        .expect("streaming ASR should await its final result");

    assert!(processing_start < final_result_wait);
    assert!(!body.contains("dictation_text_ready_processing_start"));
}

#[test]
fn clipboard_retention_takes_precedence_over_restore() {
    let mut prefs = UserPreferences::default();
    prefs.restore_clipboard_after_paste = true;
    prefs.copy_dictation_to_clipboard = true;
    assert!(!should_restore_clipboard_after_dictation(&prefs, true));
    assert!(should_restore_clipboard_after_dictation(&prefs, false));

    prefs.copy_dictation_to_clipboard = false;
    assert!(should_restore_clipboard_after_dictation(&prefs, false));
}

#[test]
fn final_clipboard_retention_cannot_hold_capsule_completion_for_seconds() {
    assert!(
        super::FINAL_CLIPBOARD_RETENTION_FOREGROUND_BUDGET <= std::time::Duration::from_millis(100)
    );
    let source = include_str!("dictation_session.rs");
    let start = source
        .find("async fn retain_final_clipboard_with_foreground_budget")
        .expect("bounded final clipboard helper should exist");
    let end = source[start..]
        .find("pub(super) async fn handle_pressed_edge")
        .map(|offset| start + offset)
        .expect("session lifecycle should follow clipboard retention helper");
    let body = &source[start..end];
    assert!(
        body.find("return (false, \"transport_held\")")
            .expect("partial paste must hold the transport clipboard")
            < body.find("spawn_blocking").expect("retention task should exist"),
        "unconfirmed partial paste must keep its clipboard payload before any retention task starts"
    );
    assert!(body.contains("spawn_blocking"));
    assert!(body.contains("tokio::time::timeout"));
    assert!(body.contains("(false, \"pending\")"));
}

#[test]
fn partial_paste_keeps_transport_clipboard_until_target_can_read_it() {
    use super::unconfirmed_paste_uses_clipboard;
    use crate::coordinator::DeliveryRoute;
    assert!(
        unconfirmed_paste_uses_clipboard(
            InsertStatus::PasteSent,
            DeliveryRoute::Paste,
            true,
            "。",
            "已经提前输出的整句话。",
        ),
    );
    assert!(
        unconfirmed_paste_uses_clipboard(
            InsertStatus::PasteSent,
            DeliveryRoute::Paste,
            true,
            "整句话",
            "整句话",
        ),
        "a prior streamed paste is still unconfirmed even if final dispatch is skipped"
    );
    assert!(!unconfirmed_paste_uses_clipboard(
            InsertStatus::PasteSent,
            DeliveryRoute::Paste,
            false,
            "整句话",
            "整句话",
        ));
    assert!(!unconfirmed_paste_uses_clipboard(
            InsertStatus::Inserted,
            DeliveryRoute::Tsf,
            true,
            "。",
            "整句话。",
        ));
}

#[test]
fn voice_activation_stop_never_counts_as_user_initiated() {
    assert!(!embedded_audio_stop_is_user_initiated(Some(
        crate::embedded_audio::SessionStopOrigin::VoiceActivation
    )));
    assert!(!embedded_audio_stop_is_user_initiated(Some(
        crate::embedded_audio::SessionStopOrigin::VoiceActivationMaxDuration
    )));
    assert!(embedded_audio_stop_is_user_initiated(Some(
        crate::embedded_audio::SessionStopOrigin::User
    )));
    assert!(embedded_audio_stop_is_user_initiated(None));
}

#[test]
fn post_dictation_key_requires_a_successful_plain_dictation_insert() {
    let enter = should_send_post_dictation_key(
        true,
        PostDictationKey::Enter,
        InsertStatus::Inserted,
        true,
        true,
        true,
        false,
    )
    .expect("inserted dictation should submit");
    assert_eq!(enter.primary, "Enter");
    assert!(enter.modifiers.is_empty());

    let ctrl_enter = should_send_post_dictation_key(
        true,
        PostDictationKey::CtrlEnter,
        InsertStatus::Inserted,
        true,
        true,
        true,
        false,
    )
    .expect("confirmed dictation should submit");
    assert_eq!(ctrl_enter.primary, "Enter");
    assert_eq!(ctrl_enter.modifiers, ["ctrl"]);

    for status in [
        InsertStatus::PasteSent,
        InsertStatus::CopiedFallback,
        InsertStatus::Failed,
    ] {
        assert!(should_send_post_dictation_key(
            true,
            PostDictationKey::Enter,
            status,
            true,
            true,
            true,
            false,
        )
        .is_none());
    }
    assert!(should_send_post_dictation_key(
        false,
        PostDictationKey::Enter,
        InsertStatus::Inserted,
        true,
        true,
        true,
        false,
    )
    .is_none());
    for denied_context in 0..3 {
        let (nonempty, target_restored, clipboard_satisfied) = match denied_context {
            0 => (false, true, true),
            1 => (true, false, true),
            _ => (true, true, false),
        };
        assert!(should_send_post_dictation_key(
            true,
            PostDictationKey::Enter,
            InsertStatus::Inserted,
            nonempty,
            target_restored,
            clipboard_satisfied,
            false,
        )
        .is_none());
    }
    assert!(should_send_post_dictation_key(
        true,
        PostDictationKey::Enter,
        InsertStatus::Inserted,
        true,
        true,
        true,
        true,
    )
    .is_none());
}

#[test]
fn post_dictation_key_claim_is_once_per_session() {
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    {
        let mut state = coordinator.inner.state.lock();
        state.session_id = session_id;
        state.phase = SessionPhase::Inserting;
    }

    assert!(claim_post_dictation_key(&coordinator.inner, session_id));
    assert!(!claim_post_dictation_key(&coordinator.inner, session_id));
    assert!(!claim_post_dictation_key(
        &coordinator.inner,
        new_session_id()
    ));
}

#[test]
fn default_done_message_treats_raw_insert_after_polish_failure_as_successful_fallback() {
    // Successful type/paste: silent capsule (text already on screen).
    assert_eq!(
        default_done_message(InsertStatus::PasteSent, false, false),
        None
    );
    assert_eq!(
        default_done_message(InsertStatus::PasteSent, false, true),
        None
    );
    assert_eq!(
        default_done_message(InsertStatus::Inserted, true, true),
        None
    );
    assert_eq!(
        default_done_message(InsertStatus::PasteSent, true, true),
        None
    );
    assert_eq!(
        default_done_message(InsertStatus::Inserted, false, false),
        None
    );
    let copied_message = if cfg!(target_os = "windows") {
        "润色不可用，已复制原文，请 Ctrl+V"
    } else {
        "润色不可用，已复制原文，请粘贴"
    };
    assert_eq!(
        default_done_message(InsertStatus::CopiedFallback, true, false),
        Some(copied_message.to_string())
    );
    assert_eq!(
        default_done_message(InsertStatus::Failed, true, false),
        Some("润色不可用，插入失败".to_string())
    );
    assert_eq!(
        default_done_message(InsertStatus::Failed, true, true),
        Some(if cfg!(target_os = "windows") {
            "上屏失败，内容在剪贴板，请 Ctrl+V".to_string()
        } else {
            "上屏失败，内容在剪贴板，请粘贴".to_string()
        })
    );
}

#[test]
fn device_processing_treats_raw_insert_after_polish_failure_as_success() {
    assert!(device_processing_final_succeeded(
        InsertStatus::Inserted,
        Some("polishFailed")
    ));
    assert!(device_processing_final_succeeded(
        InsertStatus::PasteSent,
        Some("polishFailed")
    ));
    assert!(device_processing_final_succeeded(
        InsertStatus::CopiedFallback,
        Some("polishFailed")
    ));
    assert!(!device_processing_final_succeeded(
        InsertStatus::Failed,
        Some("polishFailed")
    ));
    assert!(!device_processing_final_succeeded(
        InsertStatus::Inserted,
        Some("windowsImeTsfRequired")
    ));
}

#[test]
fn device_processing_completion_delay_keeps_ai_led_visible() {
    let started_at = Instant::now();

    assert_eq!(
        device_ai_processing_completion_delay(Some(started_at), started_at),
        Duration::from_millis(DEVICE_AI_PROCESSING_MIN_VISIBLE_MS)
    );
    assert_eq!(
        device_ai_processing_completion_delay(
            Some(started_at),
            started_at + Duration::from_millis(400),
        ),
        Duration::from_millis(DEVICE_AI_PROCESSING_MIN_VISIBLE_MS - 400)
    );
    assert_eq!(
        device_ai_processing_completion_delay(
            Some(started_at),
            started_at + Duration::from_millis(DEVICE_AI_PROCESSING_MIN_VISIBLE_MS + 1),
        ),
        Duration::from_millis(0)
    );
    assert_eq!(
        device_ai_processing_completion_delay(None, started_at),
        Duration::from_millis(0)
    );
}

#[test]
fn device_processing_max_visible_timeout_is_bounded() {
    assert_eq!(DEVICE_AI_PROCESSING_MAX_VISIBLE_MS, 5_000);
    assert!(DEVICE_AI_PROCESSING_MAX_VISIBLE_MS > DEVICE_AI_PROCESSING_MIN_VISIBLE_MS);
    assert!(Duration::from_millis(DEVICE_AI_PROCESSING_MAX_VISIBLE_MS) <= Duration::from_secs(5));
}

#[test]
fn kws_hit_schedules_immediate_local_confirmation() {
    let stream = include_str!("dictation_embedded_stream.rs");
    assert!(
        stream.contains("kws_prompted_local_confirm")
            && stream.contains("kws_immediate")
            && stream.contains("kws_retry")
            && stream.contains("KWS_IMMEDIATE_LOCAL_CONFIRM_MIN_BYTES")
            && stream.contains("kws_local_absent_count")
            && stream.contains("kws_first_hit_at")
            && stream.contains("stage1 KWS hit")
            && stream.contains("PendingSecondaryDecision::AwaitSecondary")
            && !stream.contains("stage2 timeout fail-open KeywordModel")
            && stream.contains("stage2 timeout held after explicit Absent")
            && stream.contains("stage2 Absent reject")
            && !stream.contains("KWS provisional accept after local Absent"),
        "stage1 KWS schedules stage2; unfinished confirmation cannot authorize activation"
    );
    let polish = include_str!("dictation_wake_polish.rs");
    assert!(
        polish.contains("kws_prompted_local_confirm")
            && polish.contains("KWS_IMMEDIATE_LOCAL_CONFIRM_MIN_MS: usize = 700")
            && polish.contains("KWS_LOCAL_CONFIRM_RETRY_MS: usize = 400")
            && polish.contains("KWS_SECONDARY_CONFIRM_BUDGET_MS: u64 = 60")
            && polish.contains("KWS_SECONDARY_ABSENT_REJECT_COUNT: u8 = 2")
            && polish.contains("gain_normalized_pcm16"),
        "secondary budget 60ms + 2 Absent rejects + boosted (min 8x) local ASR"
    );
    assert_eq!(super::KWS_SECONDARY_CONFIRM_BUDGET_MS, 60);
    assert_eq!(super::KWS_IMMEDIATE_LOCAL_CONFIRM_MIN_MS, 700);
}

#[cfg(target_os = "windows")]
#[test]
fn explicit_absent_blocks_keyword_only_secondary_fallback() {
    assert!(super::secondary_fallback_can_accept_keyword(true, 0));
    assert!(!super::secondary_fallback_can_accept_keyword(true, 1));
    assert!(!super::secondary_fallback_can_accept_keyword(true, 2));
    assert!(!super::secondary_fallback_can_accept_keyword(false, 0));
}

#[cfg(target_os = "windows")]
#[test]
fn a_slow_unfinished_confirmation_is_not_positive_wake_evidence() {
    // Installed 2961768663: KWS arrived while an earlier confirmation was
    // still running (351 ms); elapsed work was incorrectly treated as a hit.
    for waited in [60, 351, 1_000, 4_000, u64::MAX] {
        assert_eq!(
            super::pending_secondary_decision(true, waited, 0),
            super::PendingSecondaryDecision::AwaitSecondary,
            "elapsed time cannot turn an unfinished confirmation into phrase evidence",
        );
    }
    assert!(super::local_confirmation_can_activate(
        true,
        crate::wake_phrase::LocalPhraseRelation::ExactStart,
    ));
    assert!(super::local_confirmation_can_activate(
        true,
        crate::wake_phrase::LocalPhraseRelation::PresentLater,
    ));
    assert!(super::local_confirmation_can_activate(
        false,
        crate::wake_phrase::LocalPhraseRelation::ExactStart,
    ));
}

#[cfg(target_os = "windows")]
#[test]
fn pending_secondary_keeps_positive_evidence_separate_from_latency() {
    use super::PendingSecondaryDecision::{AwaitSecondary, HoldAfterExplicitAbsent};

    assert_eq!(
        super::pending_secondary_decision(true, 59, 0),
        AwaitSecondary
    );
    assert_eq!(
        super::pending_secondary_decision(true, 60, 0),
        AwaitSecondary
    );
    assert_eq!(
        super::pending_secondary_decision(true, 60, 1),
        HoldAfterExplicitAbsent
    );
    assert_eq!(
        super::pending_secondary_decision(false, u64::MAX, 0),
        AwaitSecondary
    );
}

#[cfg(target_os = "windows")]
#[test]
fn secondary_budget_counts_pre_hit_confirmation_work_once() {
    assert_eq!(super::effective_secondary_waited_ms(0, 158), 158);
    assert_eq!(super::effective_secondary_waited_ms(36, 158), 158);
    assert_eq!(super::effective_secondary_waited_ms(100, 20), 100);
    assert_eq!(super::effective_secondary_waited_ms(99, 20), 99);
    assert_eq!(
        super::pending_secondary_decision(true, super::effective_secondary_waited_ms(0, 158), 0,),
        super::PendingSecondaryDecision::AwaitSecondary,
    );
}

#[cfg(target_os = "windows")]
#[test]
fn incomplete_pre_hit_absent_cannot_veto_a_later_keyword_hit() {
    use crate::wake_phrase::LocalPhraseRelation::Absent;

    assert_eq!(
        super::authoritative_local_absent_coverage(Absent, 3, 4, 0, 1_600 * 32),
        None,
        "a 3/4-character partial is HoldForMoreEvidence, not explicit Absent",
    );
    assert_eq!(
        super::authoritative_local_absent_coverage(Absent, 4, 4, 0, 800 * 32),
        Some(super::LocalConfirmationCoverage {
            start_bytes: 0,
            end_bytes: 800 * 32,
        }),
        "a full-length non-match remains authoritative anti-false-wake evidence",
    );
}

#[cfg(target_os = "windows")]
#[test]
fn pre_hit_absent_blocks_only_the_keyword_endpoint_it_already_covered() {
    let covered = super::LocalConfirmationCoverage {
        start_bytes: 0,
        end_bytes: 1_855 * 32,
    };
    assert!(super::local_absent_covers_keyword_endpoint(
        Some(covered),
        1_055 * 32,
        1.815,
    ));
    assert!(!super::local_absent_covers_keyword_endpoint(
        Some(covered),
        2_035 * 32,
        2.795,
    ));

    let later_window = super::LocalConfirmationCoverage {
        start_bytes: 2_035 * 32,
        end_bytes: 3_435 * 32,
    };
    assert!(super::local_absent_covers_keyword_endpoint(
        Some(later_window),
        2_035 * 32,
        2.795,
    ));
}

#[cfg(target_os = "windows")]
#[test]
fn session_643_full_absent_tail_conflict_blocks_keyword_timeout_fallback() {
    let covered = super::LocalConfirmationCoverage {
        start_bytes: 0,
        end_bytes: 839 * 32,
    };
    assert!(super::local_absent_covers_keyword_endpoint(
        Some(covered),
        0,
        1.040,
    ));
    assert_eq!(
        super::pending_secondary_decision(true, 103, 1),
        super::PendingSecondaryDecision::HoldAfterExplicitAbsent,
        "an authoritative local non-match must not be reversed by the 60 ms KWS timeout",
    );

    let session_862_covered = super::LocalConfirmationCoverage {
        start_bytes: 1_010 * 32,
        end_bytes: 2_410 * 32,
    };
    assert!(super::local_absent_covers_keyword_endpoint(
        Some(session_862_covered),
        1_010 * 32,
        2.690,
    ));

    let old_unrelated = super::LocalConfirmationCoverage {
        start_bytes: 0,
        end_bytes: 700 * 32,
    };
    assert!(!super::local_absent_covers_keyword_endpoint(
        Some(old_unrelated),
        0,
        1.040,
    ));
}

#[cfg(target_os = "windows")]
#[test]
fn completed_full_length_absent_blocks_fallback_without_rejecting_partial_phrase() {
    use crate::wake_phrase::LocalPhraseRelation;

    assert!(super::completed_secondary_absent_is_authoritative(
        LocalPhraseRelation::Absent,
        4,
        4,
    ));
    assert!(super::completed_secondary_absent_is_authoritative(
        LocalPhraseRelation::Absent,
        5,
        4,
    ));
    assert!(!super::completed_secondary_absent_is_authoritative(
        LocalPhraseRelation::Absent,
        3,
        4,
    ));
    assert!(!super::completed_secondary_absent_is_authoritative(
        LocalPhraseRelation::ExactStart,
        4,
        4,
    ));
}

#[cfg(target_os = "windows")]
#[test]
fn busy_local_wake_helper_is_retried_without_queue_or_keyword_fallback() {
    assert!(crate::asr::local::wake_helper::is_busy_error(
        "local wake confirmation failed: local_wake_helper_busy"
    ));
    assert!(!crate::asr::local::wake_helper::is_busy_error(
        "local wake helper did not start"
    ));

    let helper = include_str!("../asr/local/wake_helper.rs");
    assert!(
        helper.contains(".process\n                .try_lock()")
            && !helper.contains("let mut process_slot = self.process.lock();"),
        "local confirmations must be single-flight and non-queueing"
    );

    let stream = include_str!("dictation_embedded_stream.rs");
    let busy_branch = stream
        .find("is_busy_error(&err)")
        .expect("busy helper branch must exist");
    let unavailable_fallback = stream[busy_branch..]
        .find("secondary_fallback_can_accept_keyword(")
        .expect("ordinary helper failure fallback must remain");
    let busy_retry = stream[busy_branch..]
        .find("stage2 local confirm busy; retrying without queue")
        .expect("busy helper retry log must exist");
    assert!(
        busy_retry < unavailable_fallback,
        "busy backpressure must return before keyword-only fallback"
    );
}

#[test]
fn terminal_local_confirmation_uses_last_2500ms_of_long_candidates() {
    let short = vec![0u8; 1_000 * 32];
    let (pcm, origin) = super::terminal_local_confirmation_pcm(&short);
    assert_eq!(pcm.len(), short.len());
    assert_eq!(origin, 0);

    let mut long = vec![1u8; 5_000 * 32];
    long[4_000 * 32] = 7;
    let (pcm, origin) = super::terminal_local_confirmation_pcm(&long);
    assert_eq!(origin, (5_000 - 2_500) * 32);
    assert_eq!(pcm.len(), 2_500 * 32);
    assert_eq!(pcm[0], 1);
    let windows = super::terminal_local_confirmation_windows(&long);
    assert_eq!(windows.len(), 3);
    assert_eq!(windows[0].1, (5_000 - 2_500) * 32);
    assert_eq!(windows[1].1, 0);
    assert_eq!(windows[1].0.len(), 2_500 * 32);
    assert_eq!(windows[2].0.len(), long.len());
}

#[test]
fn terminal_offline_recall_stops_after_initial_plus_focused_absence() {
    assert!(!super::should_run_terminal_offline_recall(
        super::MIN_TERMINAL_OFFLINE_PCM_BYTES - 2,
        0,
        false,
        false,
    ));
    assert!(super::should_run_terminal_offline_recall(
        super::MIN_TERMINAL_OFFLINE_PCM_BYTES,
        super::TERMINAL_OFFLINE_SKIP_ABSENT_COUNT - 1,
        false,
        false,
    ));
    assert!(
        super::should_run_terminal_offline_recall(
            super::MIN_TERMINAL_OFFLINE_PCM_BYTES,
            super::TERMINAL_OFFLINE_SKIP_ABSENT_COUNT,
            false,
            false,
        ),
        "early pre-roll Absents must not skip last-chance full-buffer confirm"
    );
    assert!(super::should_run_terminal_offline_recall(
        super::MIN_TERMINAL_OFFLINE_PCM_BYTES,
        u8::MAX,
        true,
        false,
    ));
    assert!(super::should_run_terminal_offline_recall(
        super::MIN_TERMINAL_OFFLINE_PCM_BYTES,
        u8::MAX,
        false,
        true,
    ));

    let stream = include_str!("dictation_embedded_stream.rs");
    assert!(
        stream.contains("Duration::from_millis(TERMINAL_OFFLINE_RECALL_BUDGET_MS)")
            && stream.contains("terminal offline recall released actor after bounded wait")
            && stream.contains("terminal skip offline cascade reason={}")
            && stream.contains("candidate.local_absent_count.saturating_add(1)"),
        "terminal offline recovery must remain bounded; repeated focused Absents suppress ambient work but not one owner-backed independent KWS check"
    );
    assert_eq!(super::TERMINAL_OFFLINE_RECALL_BUDGET_MS, 500);
}

#[cfg(target_os = "windows")]
#[test]
fn phonetic_near_match_requires_independent_kws_and_never_wakes_alone() {
    let near = super::LocalWakeConfirmation {
        matched: false,
        phrase_relation: crate::wake_phrase::LocalPhraseRelation::Absent,
        transcript_chars: 3,
        phonetic_prefix_units: 0,
        phonetic_suffix_units: 0,
        phonetic_best_distance: 1,
        phonetic_best_window_start: 0,
        inference_ms: 100,
        snapshot_pcm_ms: 1_400,
        recovered_keyword_end_seconds: None,
    };
    assert!(super::phonetic_near_phrase_evidence(&near, 4));

    let too_far = super::LocalWakeConfirmation {
        phonetic_best_distance: 2,
        ..near
    };
    assert!(!super::phonetic_near_phrase_evidence(&too_far, 4));

    let too_short = super::LocalWakeConfirmation {
        transcript_chars: 2,
        ..near
    };
    assert!(!super::phonetic_near_phrase_evidence(&too_short, 4));
    assert!(super::enrolled_terminal_kws_can_accept_phonetic_near(
        true, &near, 4
    ));
    assert!(
        !super::enrolled_terminal_kws_can_accept_phonetic_near(false, &near, 4),
        "a non-owner KWS hit must not use phonetic-near recovery"
    );
    assert!(
        !super::enrolled_terminal_kws_can_accept_phonetic_near(true, &too_far, 4),
        "owner voiceprint alone must not relax a non-near transcript"
    );

    use super::TerminalInflightLocalDecision::{AcceptLocal, PreserveKwsFusion, RecordAbsent};
    let exact = super::LocalWakeConfirmation {
        matched: true,
        phrase_relation: crate::wake_phrase::LocalPhraseRelation::ExactStart,
        transcript_chars: 4,
        phonetic_prefix_units: 4,
        phonetic_suffix_units: 0,
        phonetic_best_distance: 0,
        phonetic_best_window_start: 0,
        inference_ms: 100,
        snapshot_pcm_ms: 1_400,
        recovered_keyword_end_seconds: None,
    };
    assert_eq!(
        super::terminal_inflight_local_decision(&exact, false, 4),
        AcceptLocal
    );

    let later = super::LocalWakeConfirmation {
        phrase_relation: crate::wake_phrase::LocalPhraseRelation::PresentLater,
        ..exact
    };
    // 2026-09-20: PresentLater no longer waits for a KWS hit at the terminal
    // in-flight boundary either — the caller's arbitration is the safety gate.
    assert_eq!(
        super::terminal_inflight_local_decision(&later, false, 4),
        AcceptLocal
    );
    assert_eq!(
        super::terminal_inflight_local_decision(&later, true, 4),
        AcceptLocal
    );
    assert_eq!(
        super::terminal_inflight_local_decision(&near, false, 4),
        PreserveKwsFusion
    );
    assert_eq!(
        super::terminal_inflight_local_decision(&too_far, false, 4),
        RecordAbsent
    );
}

#[cfg(target_os = "windows")]
#[test]
fn terminal_owner_local_near_recovery_matches_installed_session_501_without_broadening_wake() {
    let installed_session_501 = super::LocalWakeConfirmation {
        matched: false,
        phrase_relation: crate::wake_phrase::LocalPhraseRelation::Absent,
        transcript_chars: 15,
        phonetic_prefix_units: 3,
        phonetic_suffix_units: 0,
        phonetic_best_distance: 1,
        phonetic_best_window_start: 0,
        inference_ms: 872,
        snapshot_pcm_ms: 5_000,
        recovered_keyword_end_seconds: None,
    };
    assert!(super::enrolled_terminal_local_near_can_accept(
        true,
        &installed_session_501,
        4,
        0,
    ));
    assert!(!super::enrolled_terminal_local_near_can_accept(
        false,
        &installed_session_501,
        4,
        0,
    ));
    assert!(!super::enrolled_terminal_local_near_can_accept(
        true,
        &installed_session_501,
        4,
        1_020 * 32,
    ));

    let only_said_incomplete_phrase = super::LocalWakeConfirmation {
        transcript_chars: 3,
        ..installed_session_501
    };
    assert!(!super::enrolled_terminal_local_near_can_accept(
        true,
        &only_said_incomplete_phrase,
        4,
        0,
    ));
    let two_units_wrong = super::LocalWakeConfirmation {
        phonetic_prefix_units: 2,
        phonetic_suffix_units: 0,
        phonetic_best_distance: 2,
        ..installed_session_501
    };
    assert!(!super::enrolled_terminal_local_near_can_accept(
        true,
        &two_units_wrong,
        4,
        0,
    ));
    let phrase_like_text_later = super::LocalWakeConfirmation {
        phonetic_best_window_start: 1,
        ..installed_session_501
    };
    assert!(!super::enrolled_terminal_local_near_can_accept(
        true,
        &phrase_like_text_later,
        4,
        0,
    ));
}

#[cfg(target_os = "windows")]
#[test]
fn terminal_open_near_recovery_matches_session_2274297156_gated_by_voiceprint_floor() {
    // 2026-09-20 08:37:58: the owner's accented wake was transcribed
    // "开su音那你看一下…" — prefix 1, distance 2, head-aligned, body text —
    // with a drifted bank that could not enroll-match. Every enrolled tier
    // was dead, the only window containing the phrase was rejected, and the
    // user had to repeat ("wake too slow"). The open tier's only bystander
    // guard is the voiceprint floor (media-only windows read 0.01-0.11).
    let accented_owner_wake = super::LocalWakeConfirmation {
        matched: false,
        phrase_relation: crate::wake_phrase::LocalPhraseRelation::Absent,
        transcript_chars: 21,
        phonetic_prefix_units: 1,
        phonetic_suffix_units: 1,
        phonetic_best_distance: 2,
        phonetic_best_window_start: 0,
        inference_ms: 640,
        snapshot_pcm_ms: 5_000,
        recovered_keyword_end_seconds: None,
    };
    let drifted_owner_voice = Ok(crate::speaker_verification::VerificationResult {
        matched: false,
        owner_matched: false,
        score: 0.27,
        policy: crate::speaker_verification::VerificationPolicy::Enrolled,
    });
    assert!(super::open_terminal_local_near_can_accept(
        &drifted_owner_voice,
        &accented_owner_wake,
        4,
        0,
    ));

    // Media near-misses ("开始上课…") share the phonetic shape; only the
    // voiceprint floor separates them.
    let media_voice = crate::speaker_verification::VerificationResult {
        score: 0.08,
        ..drifted_owner_voice.clone().unwrap()
    };
    assert!(!super::open_terminal_local_near_can_accept(
        &Ok(media_voice),
        &accented_owner_wake,
        4,
        0,
    ));
    let just_below_floor = crate::speaker_verification::VerificationResult {
        score: 0.19,
        ..drifted_owner_voice.clone().unwrap()
    };
    assert!(!super::open_terminal_local_near_can_accept(
        &Ok(just_below_floor),
        &accented_owner_wake,
        4,
        0,
    ));
    // Verifier unavailable fails closed.
    assert!(!super::open_terminal_local_near_can_accept(
        &Err("verify unavailable".to_string()),
        &accented_owner_wake,
        4,
        0,
    ));
    // Buried near-miss, tail confirm window, and distance 3 all stay out.
    let buried = super::LocalWakeConfirmation {
        phonetic_best_window_start: 2,
        ..accented_owner_wake
    };
    assert!(!super::open_terminal_local_near_can_accept(
        &drifted_owner_voice,
        &buried,
        4,
        0,
    ));
    assert!(!super::open_terminal_local_near_can_accept(
        &drifted_owner_voice,
        &accented_owner_wake,
        4,
        80_000 * 32,
    ));
    // Sessions 2274299279/2274299280 (2026-09-21 10:16): a bare accented
    // phrase with no body used to be a guaranteed terminal reject — the body
    // requirement was unsatisfiable inside a closed window and the capsule
    // only appeared when the user's repeat opened a fresh window. At terminal
    // the phrase-only transcript hearing the whole phrase may accept under
    // the same floor; losing two of four units stays out.
    let no_body = super::LocalWakeConfirmation {
        transcript_chars: 4,
        ..accented_owner_wake
    };
    assert!(super::open_terminal_local_near_can_accept(
        &drifted_owner_voice,
        &no_body,
        4,
        0,
    ));
    // Installed 2026-09-23 22:19: an unrelated five-character sentence had
    // distance 2 and zero matching wake-prefix units. The low owner floor
    // alone must not promote it into a wake.
    let unrelated_short_sentence = super::LocalWakeConfirmation {
        transcript_chars: 5,
        phonetic_prefix_units: 0,
        phonetic_suffix_units: 0,
        ..accented_owner_wake
    };
    assert!(!super::open_terminal_local_near_can_accept(
        &Ok(crate::speaker_verification::VerificationResult {
            score: 0.2186,
            ..drifted_owner_voice.clone().unwrap()
        }),
        &unrelated_short_sentence,
        4,
        0,
    ));
    let clipped_no_body = super::LocalWakeConfirmation {
        transcript_chars: 2,
        ..accented_owner_wake
    };
    assert!(!super::open_terminal_local_near_can_accept(
        &drifted_owner_voice,
        &clipped_no_body,
        4,
        0,
    ));
    let too_far = super::LocalWakeConfirmation {
        phonetic_best_distance: 3,
        ..accented_owner_wake
    };
    assert!(!super::open_terminal_local_near_can_accept(
        &drifted_owner_voice,
        &too_far,
        4,
        0,
    ));
}

#[cfg(target_os = "windows")]
#[test]
fn repeated_start_aligned_half_phrase_requires_enrolled_owner_for_overlap_recovery() {
    let overlap = super::LocalWakeConfirmation {
        matched: false,
        phrase_relation: crate::wake_phrase::LocalPhraseRelation::Absent,
        transcript_chars: 5,
        phonetic_prefix_units: 2,
        phonetic_suffix_units: 0,
        phonetic_best_distance: 2,
        phonetic_best_window_start: 0,
        inference_ms: 205,
        snapshot_pcm_ms: 2_439,
        recovered_keyword_end_seconds: None,
    };
    assert!(!super::overlap_degraded_owner_phrase_evidence(
        &overlap, 4, 0,
    ));
    let installed_session_1282_suffix_crop = super::LocalWakeConfirmation {
        transcript_chars: 11,
        phonetic_prefix_units: 0,
        phonetic_suffix_units: 2,
        phonetic_best_distance: 2,
        phonetic_best_window_start: 0,
        snapshot_pcm_ms: 4_700,
        ..overlap
    };
    assert!(super::overlap_degraded_owner_phrase_evidence(
        &installed_session_1282_suffix_crop,
        4,
        0,
    ));
    assert!(!super::overlap_degraded_owner_phrase_evidence(
        &overlap,
        4,
        1_000 * 32,
    ));
    let too_far = super::LocalWakeConfirmation {
        phonetic_best_distance: 3,
        ..overlap
    };
    assert!(!super::overlap_degraded_owner_phrase_evidence(
        &too_far, 4, 0,
    ));
    assert!(!super::enrolled_owner_repeated_overlap_near_can_accept(
        false, 3,
    ));
    assert!(!super::enrolled_owner_repeated_overlap_near_can_accept(
        true, 2,
    ));
    assert!(super::enrolled_owner_repeated_overlap_near_can_accept(
        true, 3,
    ));
}

#[test]
fn terminal_wait_budget_gives_the_inflight_5s_confirm_time_to_finish() {
    assert_eq!(super::terminal_inflight_confirmation_remaining_ms(0), 1_200);
    assert_eq!(
        super::terminal_inflight_confirmation_remaining_ms(283),
        1_200
    );
    assert_eq!(
        super::terminal_inflight_confirmation_remaining_ms(999),
        1_200
    );
}

#[test]
fn known_good_contracts_with_2026_09_13_explicit_wake_requirement() {
    assert_eq!(
        crate::speech_decision_kernel::arbitrate_wake(
            denzic_voice_activation_v1_core::PhraseSignal::None,
            crate::speech_decision_kernel::OwnerAccessEvidence::EnrolledMatch,
            true,
        )
        .decision,
        denzic_voice_activation_v1_core::GateDecision::Reject
    );
    assert_eq!(
        super::target_speaker_end_timeout_ms_for_preview(Some("今天下午三点开会")),
        super::EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS
    );
    let body = "开始录音今天下午三点开会然后我们把方案再过一遍如果没问题就按这个执行";
    let stripped = super::strip_automatic_activation_prefix(body, "开始录音", false);
    assert!(
        stripped.chars().count() > 8,
        "wake-prefix strip must not swallow the spoken body"
    );
    let mut candidates = product_final_candidates("");
    candidates.target_filter_required = true;
    candidates.partial_preview = Some(crate::asr::RawTranscript {
        text: "今天下午三点开会然后我们把方案再过一遍".into(),
        duration_ms: 8_000,
    });
    let decision = super::arbitrate_product_final_transcript(candidates, &[], false);
    assert_eq!(
        decision.transcript.text, "",
        "preview-only recovery is blocked until owner filtering supplies evidence"
    );
    let preview_src = include_str!("dictation_preview.rs");
    assert!(
        preview_src.contains("kept spoken body after stop before visual latch"),
        "late stop must not swallow a spoken body that never latched visually"
    );
}

#[test]
fn terminal_consumes_the_running_confirmation_before_applying_absent_skip() {
    let stream = include_str!("dictation_embedded_stream.rs");
    let terminal = stream
        .find("async fn finish_buffered_speaker_candidate")
        .expect("terminal candidate handler");
    let body = &stream[terminal..];
    let join = body
        .find("terminal joining in-flight local confirmation")
        .expect("terminal in-flight join");
    let skip = body
        .find("should_run_terminal_offline_recall(")
        .expect("terminal offline skip decision");
    assert!(
        join < skip,
        "an already-running final-window confirmation must finish before older Absent evidence can skip terminal recall"
    );
    assert!(body.contains("terminal_inflight_confirmation_remaining_ms(elapsed_ms)"));
    assert!(body.contains("terminal_completed_local_confirmation"));
}

#[test]
fn automatic_wake_keeps_exact_keyword_segment_but_bounds_local_transcript_fallback() {
    // Keyword-model matches preserve their known segment start so the cloud
    // does not begin halfway through the wake phrase and swallow the first body
    // words. Local-transcript recovery has no start boundary and must retain
    // only the historical bounded tail.
    assert_eq!(super::post_wake_pcm_offset_bytes(0.0, 32_000), 0);
    assert_eq!(
        super::post_wake_pcm_offset_bytes(10.24, 400_000),
        ((10.24_f32 + 0.12) * 32_000.0) as usize
    );
    assert_eq!(
        super::wake_speaker_anchor_pcm_offset_bytes(None, 0.76, 64_000),
        0
    );
    assert_eq!(
        super::wake_speaker_anchor_pcm_offset_bytes(None, 3.255, 128_000),
        ((3.255_f32 * 32_000.0).round() as usize - 800 * 32) & !1usize
    );
    assert_eq!(
        super::wake_speaker_anchor_pcm_offset_bytes(Some(0.24), 1.84, 100_000),
        120 * 32
    );
    assert_eq!(
        super::wake_speaker_anchor_pcm_offset_bytes(Some(2.0), 1.84, 100_000),
        ((1.84_f32 * 32_000.0).round() as usize - 800 * 32) & !1usize
    );
    let stream = include_str!("dictation_embedded_stream.rs");
    assert!(
        stream.contains("wake_match.start_seconds,")
            && !stream.contains(
                "phrase_signal == denzic_voice_activation_v1_core::PhraseSignal::KeywordModel {\n                    ((wake_match.end_seconds"
            ),
        "keyword start must reach the ASR anchor without reintroducing the old phrase-signal branch"
    );
}

#[cfg(target_os = "windows")]
#[test]
fn exact_phrase_only_local_confirmation_refines_late_keyword_boundary() {
    let confirmation = super::LocalWakeConfirmation {
        matched: true,
        phrase_relation: crate::wake_phrase::LocalPhraseRelation::ExactStart,
        transcript_chars: 4,
        phonetic_prefix_units: 4,
        phonetic_suffix_units: 0,
        phonetic_best_distance: 0,
        phonetic_best_window_start: 0,
        inference_ms: 100,
        snapshot_pcm_ms: 2_775,
        recovered_keyword_end_seconds: None,
    };
    let refined = super::refined_wake_end_seconds(1.915, &confirmation, 4);
    assert!((refined - 2.655).abs() < 0.001);
    assert_eq!(super::post_wake_pcm_offset_bytes(refined, 98_400), 88_800);

    let with_body = super::LocalWakeConfirmation {
        transcript_chars: 11,
        ..confirmation
    };
    assert_eq!(super::refined_wake_end_seconds(0.685, &with_body, 4), 0.685);
}

#[cfg(target_os = "windows")]
#[test]
fn local_only_second_chance_accepts_present_later_without_waiting_for_kws() {
    use crate::wake_phrase::LocalPhraseRelation;

    assert!(super::local_confirmation_can_activate(
        false,
        LocalPhraseRelation::ExactStart
    ));
    assert!(super::local_confirmation_can_activate(
        false,
        LocalPhraseRelation::PhoneticStart
    ));
    // 2026-09-20 (session 2274297156 family): holding PresentLater for a KWS
    // hit only delayed the same acceptance to the terminal pass. The contained
    // phrase now releases immediately; voiceprint arbitration stays at the
    // caller.
    assert!(super::local_confirmation_can_activate(
        false,
        LocalPhraseRelation::PresentLater
    ));
    assert!(super::local_confirmation_can_activate(
        true,
        LocalPhraseRelation::PresentLater
    ));
}

#[test]
fn kws_local_confirmation_uses_only_an_aligned_five_second_tail() {
    let short = vec![7u8; super::KWS_LOCAL_CONFIRM_MAX_PCM_BYTES - 2];
    assert_eq!(super::local_confirmation_pcm(&short, true), short);

    let long: Vec<u8> = (0..super::KWS_LOCAL_CONFIRM_MAX_PCM_BYTES + 102)
        .map(|index| (index % 251) as u8)
        .collect();
    let bounded = super::local_confirmation_pcm(&long, true);
    assert_eq!(bounded.len(), super::KWS_LOCAL_CONFIRM_MAX_PCM_BYTES);
    assert_eq!(
        bounded,
        long[long.len() - super::KWS_LOCAL_CONFIRM_MAX_PCM_BYTES..]
    );
    assert_eq!(
        super::local_confirmation_pcm(&long, false),
        long,
        "exploratory local confirmation keeps the full candidate"
    );
}

#[test]
fn kws_phrase_focus_tail_is_shorter_than_five_second_cap() {
    let long: Vec<u8> = (0..super::KWS_LOCAL_CONFIRM_MAX_PCM_BYTES + 200)
        .map(|index| (index % 251) as u8)
        .collect();
    let focus = super::kws_phrase_focus_pcm(&long);
    assert_eq!(focus.len(), super::KWS_LOCAL_CONFIRM_FOCUS_PCM_BYTES);
    assert!(focus.len() < super::KWS_LOCAL_CONFIRM_MAX_PCM_BYTES);
    assert_eq!(
        focus,
        long[long.len() - super::KWS_LOCAL_CONFIRM_FOCUS_PCM_BYTES..]
    );
    let short = vec![9u8; 800];
    assert_eq!(super::kws_phrase_focus_pcm(&short), short);
}

#[test]
fn kws_absent_hard_reject_waits_for_post_hit_phrase_horizon() {
    // First hit at 1920 ms (session-288 style): Absents before +1 s must not
    // burn the two-Absent reject budget; later Absents remain authoritative.
    assert!(!super::kws_absent_counts_toward_reject(Some(1_920), 1_800));
    assert!(!super::kws_absent_counts_toward_reject(Some(1_920), 2_040));
    assert!(!super::kws_absent_counts_toward_reject(Some(1_920), 2_900));
    assert!(super::kws_absent_counts_toward_reject(Some(1_920), 2_920));
    assert!(super::kws_absent_counts_toward_reject(Some(1_920), 3_900));
    assert!(
        super::kws_absent_counts_toward_reject(None, 800),
        "without a recorded hit, Absent evidence stays authoritative"
    );
}

#[cfg(target_os = "windows")]
#[test]
fn local_only_start_phrase_has_a_bounded_nonzero_audio_endpoint() {
    let confirmation = super::LocalWakeConfirmation {
        matched: true,
        phrase_relation: crate::wake_phrase::LocalPhraseRelation::ExactStart,
        transcript_chars: 9,
        phonetic_prefix_units: 4,
        phonetic_suffix_units: 0,
        phonetic_best_distance: 0,
        phonetic_best_window_start: 0,
        inference_ms: 100,
        snapshot_pcm_ms: 1_800,
        recovered_keyword_end_seconds: None,
    };
    let estimated = super::refined_wake_end_seconds(0.0, &confirmation, 4);
    assert!((estimated - 0.8).abs() < 0.001);

    let recovered = super::LocalWakeConfirmation {
        recovered_keyword_end_seconds: Some(0.72),
        ..confirmation
    };
    assert!((super::refined_wake_end_seconds(0.0, &recovered, 4) - 0.72).abs() < 0.001);

    let later_keyword_occurrence = super::LocalWakeConfirmation {
        recovered_keyword_end_seconds: Some(1.84),
        ..confirmation
    };
    assert!(
        (super::refined_wake_end_seconds(0.0, &later_keyword_occurrence, 4) - 0.8).abs() < 0.001
    );

    let later = super::LocalWakeConfirmation {
        phrase_relation: crate::wake_phrase::LocalPhraseRelation::PresentLater,
        recovered_keyword_end_seconds: None,
        ..confirmation
    };
    assert_eq!(super::refined_wake_end_seconds(0.0, &later, 4), 0.0);
}

#[cfg(target_os = "windows")]
#[test]
fn completed_local_confirmation_does_not_run_redundant_boundary_kws() {
    let source = include_str!("dictation_wake_polish.rs");
    let start = source
        .find("fn spawn_local_wake_confirmation(")
        .expect("local confirmation task");
    let body = &source[start..];
    let end = body
        .find("\nstruct EmbeddedStreamingDictation")
        .expect("local confirmation task end");
    let body = &body[..end];

    assert!(body.contains("result.recovered_keyword_end_seconds = None"));
    assert!(
        !body.contains("crate::wake_phrase::detect("),
        "a completed local transcript must not pay another blocking KWS pass"
    );
}

/// 2026-09-23 17:30 会话 569：活窗词 1.52s 说完，0.8/1.8/2.0s 三档 stage2
/// 全被上一隐藏窗 terminal-inflight 梯子占住单飞 helper，第一拍拖到 3.1s。
/// 让位契约三件套必须同时在场：terminal 窗口边界检查饿死计数、活窗分支
/// 才放宽 busy 预算、guard 只在 streaming-* 分支挂计数。
#[test]
fn terminal_ladder_yields_to_starving_live_stage2_confirmation() {
    let source = include_str!("dictation_wake_polish.rs");

    let terminal_start = source
        .find("async fn confirm_terminal_local_windows(")
        .expect("terminal window ladder");
    let terminal_body = &source[terminal_start..];
    let terminal_end = terminal_body
        .find("\nfn spawn_local_wake_confirmation(")
        .unwrap_or(terminal_body.len());
    let terminal_body = &terminal_body[..terminal_end];
    assert!(
        terminal_body.contains("WAKE_LIVE_STAGE2_STARVING.load"),
        "the terminal ladder must check the live starvation counter at each window boundary"
    );
    assert!(
        terminal_body.contains("TERMINAL_CONFIRM_YIELD_TO_LIVE_MS"),
        "the yield must be bounded so a leaked counter cannot starve terminal decisions"
    );

    let confirm_start = source
        .find("fn run_local_wake_confirmation_once(")
        .expect("single confirmation entry");
    let confirm_body = &source[confirm_start..];
    let confirm_end = confirm_body
        .find("\nfn spawn_local_wake_confirmation(")
        .unwrap_or(confirm_body.len());
    let confirm_body = &confirm_body[..confirm_end];
    assert!(
        confirm_body.contains("let is_live_stream_branch = context.branch.starts_with(\"streaming-\")"),
        "starvation counting must be scoped to live streaming branches"
    );
    assert!(
        confirm_body.contains("LiveStage2StarvingGuard::arm()"),
        "a busy-rejected live confirm must arm the RAII starvation guard"
    );
    assert!(
        confirm_body.contains("TERMINAL_CONFIRM_YIELD_TO_LIVE_MS.max(LOCAL_WAKE_HELPER_BUSY_RETRY_BUDGET_MS)"),
        "the live busy budget must cover the handover window so yielding can actually hand over"
    );

    // 计数器自身：arm 抬升、drop 归零（任何提前 return 都不卡高位）。
    let before = super::WAKE_LIVE_STAGE2_STARVING.load(std::sync::atomic::Ordering::Relaxed);
    {
        let _guard = super::LiveStage2StarvingGuard::arm();
        assert_eq!(
            super::WAKE_LIVE_STAGE2_STARVING.load(std::sync::atomic::Ordering::Relaxed),
            before + 1
        );
    }
    assert_eq!(
        super::WAKE_LIVE_STAGE2_STARVING.load(std::sync::atomic::Ordering::Relaxed),
        before
    );
}

#[test]
fn device_processing_max_visible_timeout_stops_without_done() {
    // Long ASR/polish: max-visible must only clear purple AI. PROCESSING:DONE is the
    // green OK flash and must fire once at real completion — not again at the 5s cap.
    let source = include_str!("dictation_device_ai.rs");
    let begin = source
        .find("fn schedule_device_ai_processing_max_visible_timeout")
        .expect("max-visible scheduler");
    let body = &source[begin..];
    let end = body[1..].find("\nfn ").map(|i| i + 1).unwrap_or(body.len());
    let body = &body[..end];
    assert!(
        body.contains("send_recording_processing_state(false"),
        "max-visible must STOP AI LED"
    );
    assert!(
        !body.contains("send_recording_processing_done"),
        "max-visible must not send DONE (avoids intermittent double green OK)"
    );
}

#[test]
fn embedded_ble_processing_sync_can_be_disabled_by_env() {
    let previous = std::env::var_os(EMBEDDED_BLE_DISABLE_PROCESSING_SYNC_ENV);
    std::env::set_var(EMBEDDED_BLE_DISABLE_PROCESSING_SYNC_ENV, "1");
    assert!(embedded_ble_processing_sync_disabled());
    std::env::set_var(EMBEDDED_BLE_DISABLE_PROCESSING_SYNC_ENV, "false");
    assert!(!embedded_ble_processing_sync_disabled());
    match previous {
        Some(value) => std::env::set_var(EMBEDDED_BLE_DISABLE_PROCESSING_SYNC_ENV, value),
        None => std::env::remove_var(EMBEDDED_BLE_DISABLE_PROCESSING_SYNC_ENV),
    }
}

#[test]
fn unit_tests_never_send_device_ai_processing_side_effects() {
    assert!(!device_ai_processing_io_allowed());
}

#[test]
fn wayland_clipboard_failure_uses_specific_error_code() {
    assert_eq!(
        dictation_error_code(InsertStatus::Failed, false, false, true, true),
        Some("waylandClipboardWriteFailed")
    );
}

#[test]
fn embedded_pcm_host_path_never_boosts_quiet_firmware_pcm() {
    let pcm = pcm_from_samples(&vec![320i16; 1_600]);
    let (limited, stats) = normalize_embedded_pcm_for_asr(&pcm);

    assert_eq!(limited, pcm);
    assert_eq!(stats.gain, 1.0);
    assert_eq!(stats.rms_after, stats.rms_before);
    assert_eq!(stats.peak_after, stats.peak_before);
    assert_eq!(stats.upstream_clipped_samples, 0);
    assert_eq!(stats.clipped_samples, 0);
}

#[test]
fn embedded_pcm_host_limiter_flags_upstream_clip_without_creating_one() {
    let mut samples = vec![30_000i16; 1_600];
    samples[0] = i16::MAX;
    samples[1] = i16::MIN;
    let pcm = pcm_from_samples(&samples);
    let (limited, stats) = normalize_embedded_pcm_for_asr(&pcm);
    let (_, peak_after) = embedded_pcm_rms_and_peak(&limited);

    assert_eq!(limited.len(), pcm.len());
    assert!(stats.gain < 1.0);
    assert!(stats.limiter_reduction_db > 0.0);
    assert_eq!(stats.upstream_clipped_samples, 2);
    assert_eq!(stats.clipped_samples, 0);
    assert!(peak_after as f64 <= EMBEDDED_AUDIO_HOST_LIMITER_PEAK.ceil());
}

#[test]
fn volcengine_streaming_path_is_attenuation_only_and_unbuffered() {
    let quiet = pcm_from_samples(&vec![12i16; 320]);
    let voice = pcm_from_samples(&vec![320i16; 320]);
    let mut state = EmbeddedStreamingAgcState::default();

    let (quiet_out, quiet_stats) = normalize_embedded_streaming_pcm_for_asr(&quiet, &mut state);
    let (voice_out, voice_stats) = normalize_embedded_streaming_pcm_for_asr(&voice, &mut state);

    assert_eq!(quiet_out, quiet);
    assert_eq!(voice_out, voice);
    assert_eq!(quiet_stats.gain, 1.0);
    assert_eq!(voice_stats.gain, 1.0);
    assert_eq!(state.quiet_chunks, 1);
    assert_eq!(state.voiced_chunks, 1);
    assert_eq!(state.gain_update_count, 0);
    assert_eq!(state.clipped_samples, 0);
}

#[test]
fn volcengine_streaming_limiter_does_not_poison_later_blocks() {
    let hot = pcm_from_samples(&vec![30_000i16; 1_600]);
    let ordinary = pcm_from_samples(&vec![3_000i16; 1_600]);
    let mut state = EmbeddedStreamingAgcState::default();

    let (_, hot_stats) = normalize_embedded_streaming_pcm_for_asr(&hot, &mut state);
    let (ordinary_out, ordinary_stats) =
        normalize_embedded_streaming_pcm_for_asr(&ordinary, &mut state);

    assert!(hot_stats.gain < 1.0);
    assert_eq!(hot_stats.clipped_samples, 0);
    assert_eq!(ordinary_stats.gain, 1.0);
    assert_eq!(ordinary_out, ordinary);
    assert_eq!(state.gain_update_count, 1);
    assert!(state.limiter_reduction_db_max > 0.0);
}

#[test]
fn volcengine_preview_and_final_share_the_authoritative_session() {
    let source = concat!(
        include_str!("dictation.rs"),
        "
",
        include_str!("dictation_preview.rs"),
        "
",
        include_str!("dictation_device_ai.rs"),
        "
",
        include_str!("dictation_wake_polish.rs"),
        "
",
        include_str!("dictation_session.rs"),
        "
",
        include_str!("dictation_embedded_submit.rs"),
        "
",
        include_str!("dictation_embedded_stream.rs"),
        "\n",
        include_str!("dictation_volcengine_callbacks.rs")
    );
    assert!(source.contains(
        "authoritative optimized-bidirectional ASR ready; preview and final share one provider session"
    ));
    assert!(source.contains("set_volcengine_preview_callbacks"));
    assert!(source.contains("asr.set_partial_transcript_callback"));
    assert!(source.contains("asr.set_final_intermediate_transcript_callback"));
    let builder_name = ["build", "_volcengine_asr("].concat();
    assert_eq!(source.matches(&builder_name).count(), 3);
}

#[test]
fn windows_volcengine_session_refreshes_external_credential_updates_before_reading() {
    let source = include_str!("../coordinator.rs");
    let reader = source
        .split("fn read_volc_credentials()")
        .nth(1)
        .expect("Volcengine credential reader must exist");
    let refresh = reader
        .find("CredentialsVault::refresh_from_system()")
        .expect("Windows sessions must refresh the OS credential document");
    let first_read = reader
        .find("CredentialsVault::get(CredentialAccount::VolcengineAppKey)")
        .expect("Volcengine App ID read must exist");
    assert!(
        refresh < first_read,
        "the external vault refresh must happen before any cached Volcengine field is read"
    );
}

#[test]
fn sustained_near_phrase_rescue_is_wired_with_rollback_and_extraction_upgrade() {
    // 2026-09-23 干扰吞唤醒修复：声纹失明时由持续近音确认兜底。
    // 三件事缺一不可：kernel 判定、env 回退、放在分离升级块之前
    // （救援放行的会话仍要进 !enrolled_owner_matched 分支尝试归属升级）。
    let stream = include_str!("dictation_embedded_stream.rs");
    let kernel = include_str!("../speech_decision_kernel.rs");
    assert!(
        kernel.contains("fn sustained_near_phrase_wake_can_activate"),
        "kernel must own the sustained near-phrase rescue predicate"
    );
    let rescue = stream
        .find("sustained_near_phrase_wake_can_activate")
        .expect("terminal gate must consult the sustained near-phrase rescue");
    let rollback = stream
        .find("LISTENER_ENABLE_SUSTAINED_NEAR_PHRASE_RESCUE")
        .expect("rescue must be opt-in after the 2026-09-23 15:37 drama-audio false wake");
    let extraction = stream
        .find("let mut owner_verified_by_extraction = false;")
        .expect("extraction upgrade block anchor must exist");
    assert!(
        rollback < rescue,
        "rollback switch must gate the rescue predicate call"
    );
    assert!(
        rescue < extraction,
        "rescue must run before the extraction block so rescued sessions still get attribution upgrade"
    );
    assert!(
        stream.contains("sustained near-phrase wake rescued (owner-blind)"),
        "rescue must log its own judgment line for decisions-watcher"
    );
}

#[test]
fn every_automatic_wake_path_seeds_session_speaker_tracking() {
    let body = concat!(
        include_str!("dictation_embedded_stream_session.rs"),
        "\n",
        include_str!("dictation_embedded_stream.rs")
    );
    assert_eq!(
        body.matches("start_local_speaker_tracking(").count(),
        3,
        "live, terminal-with-body, and terminal-continuation wake paths must bind endpointing to the wake speaker"
    );
    let wake_polish = include_str!("dictation_wake_polish.rs");
    assert!(
        wake_polish.contains("note_verified_local_speaker_tracking_started(&wake_phrase)"),
        "accepted automatic wake identity must survive into body isolation"
    );

    let continuation_take = body
        .find("take_terminal_wake_continuation(inner, session.session_id)")
        .expect("terminal continuation must attach to the next coordinator session");
    let continuation_seed = body[continuation_take..]
        .find("start_local_speaker_tracking(")
        .map(|offset| continuation_take + offset)
        .expect("terminal continuation must seed target-speaker tracking");
    let continuation_activate = body[continuation_seed..]
        .find("activate_embedded_audio_dictation_session")
        .map(|offset| continuation_seed + offset)
        .expect("terminal continuation must activate its bound session");
    assert!(continuation_take < continuation_seed && continuation_seed < continuation_activate);

    let live_accept = body
        .find("live automatic session activated and released")
        .expect("live automatic wake path must remain present");
    let live_seed = body[..live_accept]
        .rfind("start_local_speaker_tracking(")
        .expect("live automatic wake path must seed target-speaker tracking");
    let live_session = body[..live_accept]
        .rfind("begin_embedded_audio_dictation_session(inner).await?")
        .expect("live automatic wake path must create a dictation session");
    assert!(live_session < live_seed && live_seed < live_accept);
}

#[tokio::test]
async fn activation_segment_race_rebinds_post_activation_segment_instead_of_finalizing() {
    for with_body in [false, true] {
        // 2026-08-09 12:46:59 复现 fixture：VREC:ACTIVATE 后 0.1s 旧唤醒段（93）
        // complete，仅含 1845ms 唤醒词——竞态窗口内旧段 STOP 不得 finalize（否则
        // 必空稿）；听写会话必须绑定激活后的新设备段（94），其 PCM 正常进 ASR。
        let coordinator = Coordinator::new();
        let session_id = new_session_id();
        {
            let mut state = coordinator.inner.state.lock();
            state.session_id = session_id;
            state.phase = SessionPhase::Listening;
            state.cancelled = false;
        }
        register_embedded_ble_cancel_flag(&coordinator.inner, &Arc::new(AtomicBool::new(false)));
        let consumer = Arc::new(CountingConsumer::default());
        let consumer_for_session: Arc<dyn crate::recorder::AudioConsumer> = consumer.clone();
        let mut streaming = EmbeddedStreamingDictation::background_listener();
        streaming.embedded_session_id = Some(93);
        streaming.session = Some(embedded_audio_test_session(
            session_id,
            consumer_for_session,
        ));
        streaming.activation_segment_race_guard = Some((93, Instant::now()));
        if with_body {
            update_embedded_audio_partial_preview(
                &coordinator.inner,
                session_id,
                "正在说正文".into(),
            );
        }

        // 旧段在竞态窗口内 STOP：不 finalize，会话保持打开，守卫保留等待新段。
        let handled = streaming
            .handle_ble_packet_actor_command(
                &coordinator.inner,
                StreamingSessionEvent::Stopped {
                    session_id: 93,
                    expected_packet_count: 58,
                    origin: if with_body {
                        crate::embedded_audio::SessionStopOrigin::VoiceActivationMaxDuration
                    } else {
                        crate::embedded_audio::SessionStopOrigin::VoiceActivation
                    },
                },
            )
            .await
            .expect("pre-activation stop handled");
        assert!(
            !handled,
            "pre-activation segment rotation is pending, not a completed dictation"
        );
        assert!(
            streaming.session.is_some(),
            "dictation session must stay open across the pre-activation segment stop"
        );
        assert_eq!(
            streaming.activation_segment_race_guard.map(|guard| guard.0),
            Some(93),
            "race guard stays armed until the post-activation segment binds"
        );
        assert_eq!(streaming.embedded_session_id, None);
        assert_eq!(streaming.pending_stop_expected_packet_count, None);
        assert!(!streaming.terminal_received);

        // 同段尾包（极端时序）仍正常喂入——正常路径里正文就在激活后的同段延续。
        let wake_tail = pcm_from_samples(&samples_for_ms(100, 3_000));
        streaming.activation_segment_race_guard = Some((93, Instant::now()));
        streaming.embedded_session_id = Some(93);
        streaming
            .handle_ble_packet_actor_command(
                &coordinator.inner,
                StreamingSessionEvent::PcmChunk(StreamingPcmChunk {
                    session_id: 93,
                    packet_sequence: 12,
                    pcm: wake_tail.clone(),
                    raw_input_level_percent: Some(20),
                    after_stop_boundary: false,
                    metadata: None,
                }),
            )
            .await
            .expect("same-segment tail handled");
        assert_eq!(
            consumer.bytes.load(Ordering::SeqCst),
            wake_tail.len(),
            "same-segment PCM after activation must still feed ASR (normal path body)"
        );
        assert_eq!(
            streaming.activation_segment_race_guard.map(|guard| guard.0),
            Some(93),
            "same-segment PCM must not clear the race guard"
        );

        // 激活后的新设备段（94，唤醒监听窗 rotation）Started：直接绑定听写会话。
        streaming
            .handle_ble_packet_actor_command(
                &coordinator.inner,
                StreamingSessionEvent::Started {
                    session_id: 94,
                    origin: crate::embedded_audio::SessionStartOrigin::VoiceActivation,
                },
            )
            .await
            .expect("post-activation segment start handled");
        assert_eq!(streaming.embedded_session_id, Some(94));
        assert!(streaming.activation_segment_race_guard.is_none());
        assert!(streaming.session.is_some());

        // 新段正文 PCM 正常进 ASR。
        let body = pcm_from_samples(&samples_for_ms(200, 2_500));
        streaming
            .handle_ble_packet_actor_command(
                &coordinator.inner,
                StreamingSessionEvent::PcmChunk(StreamingPcmChunk {
                    session_id: 94,
                    packet_sequence: 0,
                    pcm: body.clone(),
                    raw_input_level_percent: Some(33),
                    after_stop_boundary: false,
                    metadata: None,
                }),
            )
            .await
            .expect("post-activation body handled");
        assert_eq!(
            consumer.bytes.load(Ordering::SeqCst),
            wake_tail.len() + body.len(),
            "post-activation segment body must reach ASR"
        );
    }
}

#[tokio::test]
async fn accepted_wake_ensure_binds_user_origin_continuation_without_reactivation() {
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    {
        let mut state = coordinator.inner.state.lock();
        state.session_id = session_id;
        state.phase = SessionPhase::Listening;
        state.cancelled = false;
    }
    register_embedded_ble_cancel_flag(&coordinator.inner, &Arc::new(AtomicBool::new(false)));
    let consumer = Arc::new(CountingConsumer::default());
    let consumer_for_session: Arc<dyn crate::recorder::AudioConsumer> = consumer.clone();
    let mut streaming = EmbeddedStreamingDictation::background_listener();
    streaming.embedded_session_id = Some(93);
    streaming.session = Some(embedded_audio_test_session(
        session_id,
        consumer_for_session,
    ));
    let now = Instant::now();
    streaming.activation_segment_race_guard = Some((93, now));
    streaming.accepted_wake_capture_ensure = Some(super::AcceptedWakeCaptureEnsure {
        request_id: 7,
        previous_segment_id: 93,
        requested_at: now,
        deadline_at: now + super::EMBEDDED_ACCEPTED_WAKE_CAPTURE_REPLACEMENT_TIMEOUT,
        replacement_wait_started_at: Some(now),
        confirmed_segment_id: None,
    });

    streaming
        .handle_ble_packet_actor_command(
            &coordinator.inner,
            StreamingSessionEvent::Started {
                session_id: 94,
                origin: crate::embedded_audio::SessionStartOrigin::Unknown(
                    super::embedded_ensure_start_origin_marker(7),
                ),
            },
        )
        .await
        .expect("ENSURE continuation start handled");

    assert_eq!(streaming.embedded_session_id, Some(94));
    assert!(streaming.activation_segment_race_guard.is_none());
    assert_eq!(
        streaming
            .accepted_wake_capture_ensure
            .and_then(|ensure| ensure.confirmed_segment_id),
        Some(94)
    );
    assert!(streaming.session.is_some());
}

#[tokio::test]
async fn accepted_wake_ensure_same_segment_stop_finalizes_normally() {
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    {
        let mut state = coordinator.inner.state.lock();
        state.session_id = session_id;
        state.phase = SessionPhase::Listening;
        state.cancelled = false;
    }
    register_embedded_ble_cancel_flag(&coordinator.inner, &Arc::new(AtomicBool::new(false)));
    let consumer = Arc::new(CountingConsumer::default());
    let consumer_for_session: Arc<dyn crate::recorder::AudioConsumer> = consumer.clone();
    let mut streaming = EmbeddedStreamingDictation::background_listener();
    streaming.embedded_session_id = Some(93);
    streaming.session = Some(embedded_audio_test_session(
        session_id,
        consumer_for_session,
    ));
    let now = Instant::now();
    streaming.activation_segment_race_guard = Some((93, now));
    streaming.accepted_wake_capture_ensure = Some(super::AcceptedWakeCaptureEnsure {
        request_id: 8,
        previous_segment_id: 93,
        requested_at: now,
        deadline_at: now + super::EMBEDDED_ACCEPTED_WAKE_CAPTURE_REPLACEMENT_TIMEOUT,
        replacement_wait_started_at: None,
        confirmed_segment_id: None,
    });

    streaming
        .handle_ble_packet_actor_command(
            &coordinator.inner,
            StreamingSessionEvent::Started {
                session_id: 93,
                origin: crate::embedded_audio::SessionStartOrigin::Unknown(
                    super::embedded_ensure_start_origin_marker(8),
                ),
            },
        )
        .await
        .expect("same-segment ENSURE marker handled");

    let handled = streaming
        .handle_ble_packet_actor_command(
            &coordinator.inner,
            StreamingSessionEvent::Stopped {
                session_id: 93,
                expected_packet_count: 58,
                origin: crate::embedded_audio::SessionStopOrigin::VoiceActivationMaxDuration,
            },
        )
        .await
        .expect("same-segment stop handled");

    assert!(!handled, "empty collector waits for its normal stop drain");
    assert_eq!(streaming.embedded_session_id, Some(93));
    assert!(streaming.activation_segment_race_guard.is_none());
    assert_eq!(streaming.pending_stop_expected_packet_count, Some(58));
    assert_eq!(
        streaming
            .accepted_wake_capture_ensure
            .and_then(|ensure| ensure.confirmed_segment_id),
        Some(93),
        "same-segment confirmation must remain owned until terminal cleanup"
    );
}

#[tokio::test]
async fn accepted_wake_ensure_rejects_unrelated_manual_segment() {
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    {
        let mut state = coordinator.inner.state.lock();
        state.session_id = session_id;
        state.phase = SessionPhase::Listening;
        state.cancelled = false;
    }
    register_embedded_ble_cancel_flag(&coordinator.inner, &Arc::new(AtomicBool::new(false)));
    let consumer = Arc::new(CountingConsumer::default());
    let consumer_for_session: Arc<dyn crate::recorder::AudioConsumer> = consumer.clone();
    let mut streaming = EmbeddedStreamingDictation::background_listener();
    streaming.embedded_session_id = Some(93);
    streaming.session = Some(embedded_audio_test_session(
        session_id,
        consumer_for_session,
    ));
    let now = Instant::now();
    streaming.activation_segment_race_guard = Some((93, now));
    streaming.accepted_wake_capture_ensure = Some(super::AcceptedWakeCaptureEnsure {
        request_id: 9,
        previous_segment_id: 93,
        requested_at: now,
        deadline_at: now + super::EMBEDDED_ACCEPTED_WAKE_CAPTURE_REPLACEMENT_TIMEOUT,
        replacement_wait_started_at: None,
        confirmed_segment_id: None,
    });

    let result = streaming
        .handle_ble_packet_actor_command(
            &coordinator.inner,
            StreamingSessionEvent::Started {
                session_id: 94,
                origin: crate::embedded_audio::SessionStartOrigin::User,
            },
        )
        .await;

    assert!(result.is_err(), "unrelated manual segment must not be adopted");
    assert_eq!(streaming.embedded_session_id, Some(93));
    assert_eq!(
        streaming
            .accepted_wake_capture_ensure
            .and_then(|ensure| ensure.confirmed_segment_id),
        None
    );
    assert_eq!(
        streaming.activation_segment_race_guard.map(|guard| guard.0),
        Some(93)
    );
}

#[tokio::test]
async fn accepted_wake_ensure_ignores_late_same_segment_marker_after_stop() {
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    {
        let mut state = coordinator.inner.state.lock();
        state.session_id = session_id;
        state.phase = SessionPhase::Listening;
        state.cancelled = false;
    }
    register_embedded_ble_cancel_flag(&coordinator.inner, &Arc::new(AtomicBool::new(false)));
    let consumer = Arc::new(CountingConsumer::default());
    let consumer_for_session: Arc<dyn crate::recorder::AudioConsumer> = consumer.clone();
    let mut streaming = EmbeddedStreamingDictation::background_listener();
    streaming.embedded_session_id = None;
    streaming.session = Some(embedded_audio_test_session(
        session_id,
        consumer_for_session,
    ));
    let now = Instant::now();
    streaming.activation_segment_race_guard = Some((93, now));
    streaming.accepted_wake_capture_ensure = Some(super::AcceptedWakeCaptureEnsure {
        request_id: 10,
        previous_segment_id: 93,
        requested_at: now,
        deadline_at: now + super::EMBEDDED_ACCEPTED_WAKE_CAPTURE_REPLACEMENT_TIMEOUT,
        replacement_wait_started_at: Some(now),
        confirmed_segment_id: None,
    });

    streaming
        .handle_ble_packet_actor_command(
            &coordinator.inner,
            StreamingSessionEvent::Started {
                session_id: 93,
                origin: crate::embedded_audio::SessionStartOrigin::Unknown(
                    super::embedded_ensure_start_origin_marker(10),
                ),
            },
        )
        .await
        .expect("late same-segment marker is safely ignored");

    assert_eq!(streaming.embedded_session_id, None);
    assert_eq!(
        streaming
            .accepted_wake_capture_ensure
            .and_then(|ensure| ensure.confirmed_segment_id),
        None,
        "a marker after predecessor STOP must not confirm a dead segment"
    );
    assert_eq!(
        streaming.activation_segment_race_guard.map(|guard| guard.0),
        Some(93)
    );
}

#[tokio::test]
async fn activation_segment_race_guard_does_not_break_normal_stop_paths() {
    // 守卫不得改变正常路径：窗口外（>2s）或正文已开始时，旧段 STOP 照常走
    // pending-stop/finalize 流程。
    for (guard_age, with_body, origin, label) in [
        (
            Some(Duration::from_secs(3)),
            false,
            crate::embedded_audio::SessionStopOrigin::VoiceActivationMaxDuration,
            "race window expired",
        ),
        (
            None,
            true,
            crate::embedded_audio::SessionStopOrigin::VoiceActivation,
            "body already started",
        ),
        (
            None,
            false,
            crate::embedded_audio::SessionStopOrigin::User,
            "explicit user stop before body",
        ),
        (
            None,
            true,
            crate::embedded_audio::SessionStopOrigin::User,
            "explicit user stop with body",
        ),
    ] {
        let coordinator = Coordinator::new();
        let session_id = new_session_id();
        {
            let mut state = coordinator.inner.state.lock();
            state.session_id = session_id;
            state.phase = SessionPhase::Listening;
            state.cancelled = false;
        }
        begin_embedded_audio_preview_session(&coordinator.inner, session_id);
        register_embedded_ble_cancel_flag(&coordinator.inner, &Arc::new(AtomicBool::new(false)));
        let consumer = Arc::new(CountingConsumer::default());
        let consumer_for_session: Arc<dyn crate::recorder::AudioConsumer> = consumer.clone();
        let mut streaming = EmbeddedStreamingDictation::background_listener();
        streaming.embedded_session_id = Some(93);
        streaming.session = Some(embedded_audio_test_session(
            session_id,
            consumer_for_session,
        ));
        let activated_at = guard_age
            .map(|age| Instant::now() - age)
            .unwrap_or_else(Instant::now);
        streaming.activation_segment_race_guard = Some((93, activated_at));
        if with_body {
            update_embedded_audio_partial_preview(&coordinator.inner, session_id, "正文".into());
            assert_eq!(
                current_embedded_audio_partial_preview(&coordinator.inner).as_deref(),
                Some("正文"),
                "normal-stop fixture must initialize the preview lifecycle before publishing body text"
            );
        }

        let handled = streaming
            .handle_ble_packet_actor_command(
                &coordinator.inner,
                StreamingSessionEvent::Stopped {
                    session_id: 93,
                    expected_packet_count: 58,
                    origin,
                },
            )
            .await
            .expect("stop handled");
        assert!(!handled, "{label}: normal stop path keeps waiting for tail");
        assert!(
            streaming.activation_segment_race_guard.is_none(),
            "{label}: race guard disarms once the normal path takes over"
        );
        assert_eq!(
            streaming.pending_stop_expected_packet_count,
            Some(58),
            "{label}: normal pending-stop flow armed"
        );
        assert!(streaming.session.is_some(), "{label}: session untouched");
    }
}

#[test]
fn unresolved_local_speech_hold_is_capped_two_seconds_after_confirmed_owner() {
    // F4 fixture（2026-08-09 12:47:04）：旁人连续说话，本地未归属人声持续推进，
    // 旧逻辑会把自动结束无限挂起。挂起以最后一次确认本人语音 +2s 封顶。
    let base = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("1".into()),
        target_speech_end_ms: Some(10_000),
        provider_audio_duration_ms: Some(20_000),
        audio_duration_ms: Some(20_000),
        local_speech_end_ms: Some(19_900),
        qualified_owner_speech_end_ms: None,
        qualified_owner_activity_advanced: false,
        local_speaker_classification_kind: None,
        local_speaker_signal_quality_sufficient: None,
        local_speaker_observation_end_ms: None,
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: None,
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(10_000),
        target_activity_advanced: false,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    // cap 内（本人 10s + 2s = 12s；未归属人声尾端 11s、且仍在 1.0s 窗口内 recent）
    // → 仍阻挡结束（本人说话分类滞后不受影响的通道保留）。
    let within_cap = crate::asr::volcengine::TargetSpeakerUpdate {
        provider_audio_duration_ms: Some(11_500),
        audio_duration_ms: Some(11_500),
        local_speech_end_ms: Some(11_000),
        ..base.clone()
    };
    assert!(super::has_unresolved_recent_owner_speech(
        &within_cap,
        1_000
    ));
    assert!(!super::target_speaker_endpoint_due(&within_cap));
    // cap 外（未归属人声尾端推进到 19.9s > 12s 封顶）→ 不再阻挡，端点可按
    // 1.0s 合同触发。
    assert!(!super::has_unresolved_recent_owner_speech(&base, 1_000));
    assert!(super::target_speaker_endpoint_due(&base));

    // Installed session 1494: owner ended locally at 10.9s; provider later
    // attributed room speech through 12.712s and local energy reached 13.8s.
    // The unrelated attribution must not renew the uncertain-owner allowance.
    let installed_session_1494 = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("0".into()),
        target_speech_end_ms: Some(10_322),
        provider_audio_duration_ms: Some(13_900),
        audio_duration_ms: Some(14_000),
        local_speech_end_ms: Some(13_800),
        qualified_owner_speech_end_ms: Some(10_900),
        qualified_owner_activity_advanced: true,
        local_speaker_classification_kind: Some(crate::asr::volcengine::LocalSpeakerClassificationKind::Target),
        local_speaker_signal_quality_sufficient: Some(true),
        local_speaker_observation_end_ms: Some(10_900),
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: Some(10_900),
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(12_712),
        target_activity_advanced: true,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    assert!(!super::has_unresolved_recent_owner_speech(
        &installed_session_1494,
        1_000
    ));
    assert!(super::target_speaker_endpoint_due(&installed_session_1494));
}

#[test]
fn installed_manual_909d8b72_keeps_recording_after_provider_punctuation() {
    let now = std::time::Instant::now();
    let update = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("0".into()),
        target_speech_end_ms: Some(9_372),
        provider_audio_duration_ms: Some(9_900),
        audio_duration_ms: Some(12_200),
        local_speech_end_ms: Some(11_900),
        qualified_owner_speech_end_ms: None,
        qualified_owner_activity_advanced: false,
        local_speaker_classification_kind: None,
        local_speaker_signal_quality_sufficient: None,
        local_speaker_observation_end_ms: None,
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: None,
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: false,
        stable_attributed_speech_end_ms: Some(9_372),
        target_activity_advanced: false,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    assert!(
        !super::SettledTargetEndpointClock::update_allows_endpoint(
            &update,
            false,
            Some(true),
            None,
            now - std::time::Duration::from_millis(2_500),
            now,
            true,
        ),
        "a provider period must not cut the remaining historical recording"
    );
    let quiet = crate::asr::volcengine::TargetSpeakerUpdate {
        audio_duration_ms: Some(14_900),
        ..update
    };
    assert!(super::SettledTargetEndpointClock::update_allows_endpoint(
        &quiet,
        false,
        Some(true),
        None,
        now - std::time::Duration::from_millis(2_500),
        now,
        true,
    ));
}

#[test]
fn uncertain_owner_tail_cannot_trigger_inactive_endpoint_mid_sentence() {
    // Live session 1226/77cf... had a confirmed owner watermark at 15.6s,
    // then a low-energy same-speaker window reached 17.3s with score 0.1089.
    // Cloud and local owner watermarks were equal, so the previous policy
    // incorrectly treated that Uncertain tail as silence and stopped at 17.7s.
    let update = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("0".into()),
        target_speech_end_ms: Some(15_632),
        provider_audio_duration_ms: Some(17_700),
        audio_duration_ms: Some(17_700),
        local_speech_end_ms: Some(17_300),
        qualified_owner_speech_end_ms: Some(15_600),
        qualified_owner_activity_advanced: true,
        local_speaker_classification_kind: Some(crate::asr::volcengine::LocalSpeakerClassificationKind::Target),
        local_speaker_signal_quality_sufficient: Some(true),
        local_speaker_observation_end_ms: Some(15_600),
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: Some(15_600),
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(15_632),
        target_activity_advanced: false,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    assert!(super::has_uncertain_owner_identity_tail(&update));
    let now = std::time::Instant::now();
    assert!(!super::SettledTargetEndpointClock::update_allows_endpoint(
        &update,
        false,
        Some(false),
        None,
        now - std::time::Duration::from_secs(2),
        now,
        true,
    ));
}

#[test]
fn installed_be0c_speakerless_owner_speech_rearms_terminal_preview_endpoint() {
    // Installed session be0c3e6e: the local verifier established the owner at
    // 4.6s, then several same-speaker windows became Uncertain while local
    // speech and the unattributed preview kept growing through 9.1s. The old
    // clock stayed armed at 4.6s and proposed STOP in the middle of the body.
    let started = std::time::Instant::now();
    let owner = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: None,
        target_speech_end_ms: None,
        provider_audio_duration_ms: Some(4_500),
        audio_duration_ms: Some(4_700),
        local_speech_end_ms: Some(4_600),
        qualified_owner_speech_end_ms: Some(4_600),
        qualified_owner_activity_advanced: true,
        local_speaker_classification_kind: Some(crate::asr::volcengine::LocalSpeakerClassificationKind::Target),
        local_speaker_signal_quality_sufficient: Some(true),
        local_speaker_observation_end_ms: Some(4_600),
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: Some(4_600),
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: None,
        target_activity_advanced: true,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: false,
    };
    let mut clock = super::SettledTargetEndpointClock::default();
    clock.note_visible_body_boundary(true, 29, started);
    let first_generation = clock
        .observe(&owner, true, started)
        .expect("confirmed owner arms the terminal-preview endpoint");

    let continuing_uncertain_owner = crate::asr::volcengine::TargetSpeakerUpdate {
        provider_audio_duration_ms: Some(9_000),
        audio_duration_ms: Some(9_100),
        local_speech_end_ms: Some(9_100),
        target_activity_advanced: false,
        pending_activity_advanced: false,
        ..owner.clone()
    };
    let continued_at = started + std::time::Duration::from_millis(800);
    let continued_generation = clock
        .observe(&continuing_uncertain_owner, true, continued_at)
        .expect("fresh speakerless speech from an established owner must rearm");
    assert_ne!(first_generation, continued_generation);
    assert_eq!(clock.armed_target_end_ms, Some(9_100));
    assert!(clock
        .due_update(
            first_generation,
            started + std::time::Duration::from_millis(1_000),
            super::EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS,
        )
        .is_none());
    assert!(clock
        .due_update(
            continued_generation,
            continued_at + std::time::Duration::from_millis(2_999),
            super::EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS,
        )
        .is_none());
    assert!(clock
        .due_update(
            continued_generation,
            continued_at + std::time::Duration::from_millis(3_000),
            super::EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS,
        )
        .is_some());

    let mut other_clock = super::SettledTargetEndpointClock::default();
    let other_generation = other_clock
        .observe(&owner, true, started)
        .expect("owner arms endpoint");
    let confirmed_other = crate::asr::volcengine::TargetSpeakerUpdate {
        local_non_target_speech_end_ms: Some(9_100),
        ..continuing_uncertain_owner
    };
    assert_eq!(
        other_clock.observe(&confirmed_other, true, continued_at),
        None,
        "explicit other-speaker evidence must not renew the owner clock",
    );
    assert!(other_clock
        .due_update(
            other_generation,
            started + std::time::Duration::from_millis(3_000),
            super::EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS,
        )
        .is_some());
}

#[test]
fn installed_r10_uncertain_owner_body_survives_script_pause_bystander_still_stops() {
    // Installed r10 fdc68b04 (first round on a healthy enrolled bank): the
    // wake window classified Target and set the owner boundary at 2.5s, but
    // the owner's body under continuous TTS interference stayed Uncertain
    // (0.28–0.47, signal_quality_sufficient=false) with no cloud speaker
    // row, and the provider punctuated the first clause terminal. At the
    // script's deliberate ~1s mid-sentence pause the one-second inactivity
    // clock fired `target_speaker_inactive_1000ms` mid-utterance (r8 only
    // survived because the broken bank left the wake window non-Target, so
    // this rule never armed). Uncertain continuing speech from an
    // established owner holds the endpoint through the pause, bounded by
    // the 2s identity-uncertainty wall; bystander-only windows flip to
    // NonTarget against the owner bank and keep the one-second contract.
    let started = std::time::Instant::now();
    let wake = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: None,
        target_speech_end_ms: None,
        provider_audio_duration_ms: Some(2_400),
        audio_duration_ms: Some(2_500),
        local_speech_end_ms: Some(2_500),
        qualified_owner_speech_end_ms: Some(2_500),
        qualified_owner_activity_advanced: true,
        local_speaker_classification_kind:
            Some(crate::asr::volcengine::LocalSpeakerClassificationKind::Target),
        local_speaker_signal_quality_sufficient: Some(true),
        local_speaker_observation_end_ms: Some(2_500),
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: Some(2_500),
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: None,
        target_activity_advanced: true,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: false,
    };
    let mut clock = super::SettledTargetEndpointClock::default();
    // r10: the visible preview ended terminal at the pause ("…端点复测。"),
    // so the open-clause hold is NOT available — only the owner continuation
    // hold can carry the pause.  The preview grows across the body exactly as
    // the provider streamed it in production (words arriving until the
    // pause); each growth refreshes the positive-evidence budget.
    clock.note_visible_body_boundary(false, 6, started);
    clock
        .observe(&wake, true, started)
        .expect("wake Target window arms the owner endpoint");
    clock.note_visible_body_boundary(
        false,
        10,
        started + std::time::Duration::from_millis(2_500),
    );
    clock.note_visible_body_boundary(
        true,
        13,
        started + std::time::Duration::from_millis(5_000),
    );

    // Mid-pause snapshot: provider text stalled (no advanced flags), body
    // windows Uncertain with degraded quality, speech edge 800ms behind
    // live audio, no non-target evidence anywhere.
    let uncertain_pause = crate::asr::volcengine::TargetSpeakerUpdate {
        provider_audio_duration_ms: Some(8_800),
        audio_duration_ms: Some(8_800),
        local_speech_end_ms: Some(8_000),
        qualified_owner_activity_advanced: false,
        local_speaker_classification_kind:
            Some(crate::asr::volcengine::LocalSpeakerClassificationKind::Uncertain),
        local_speaker_signal_quality_sufficient: Some(false),
        local_speaker_observation_end_ms: Some(8_000),
        local_tentative_owner_speech_end_ms: None,
        local_non_target_speech_end_ms: None,
        target_activity_advanced: false,
        pending_activity_advanced: false,
        ..wake.clone()
    };
    let pause_at = started + std::time::Duration::from_millis(6_000);
    let pause_generation = clock
        .observe(&uncertain_pause, true, pause_at)
        .expect("speakerless continuing body rearms on the live speech edge");

    // ~1s script pause: aged audio 8800+1100=9900 leaves a 1.9s gap to the
    // 8000ms speech edge — still inside the 2s uncertainty budget (and the
    // probe is inside the 3s wall clock), so the deliberate pause must not
    // stop the session.
    assert!(clock
        .due_update(
            pause_generation,
            pause_at + std::time::Duration::from_millis(1_100),
            super::EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS,
        )
        .is_none());

    // A pause that outruns both walls is a real ending: at +3s the aged gap
    // is 3.8s (past the 2s uncertainty budget) and the 3s endpoint wall
    // clock since the rearm has fully elapsed.
    assert!(clock
        .due_update(
            pause_generation,
            pause_at + std::time::Duration::from_millis(3_000),
            super::EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS,
        )
        .is_some());

    // Bystander takeover: the newest windows flip to NonTarget against the
    // owner bank (installed r10 post-stop scored 0.01–0.11), so the same
    // snapshot with explicit non-target evidence stops at the ordinary
    // one-second contract. Non-target evidence deliberately does not rearm
    // the owner clock (`should_rearm` blocks it), so the wake generation
    // stays authoritative and its deadline runs out normally.
    let mut other_clock = super::SettledTargetEndpointClock::default();
    other_clock.note_visible_body_boundary(true, 13, started);
    let wake_generation = other_clock
        .observe(&wake, true, started)
        .expect("owner arms endpoint");
    let bystander_tail = crate::asr::volcengine::TargetSpeakerUpdate {
        local_non_target_speech_end_ms: Some(8_000),
        ..uncertain_pause.clone()
    };
    assert!(
        other_clock
            .observe(&bystander_tail, true, pause_at)
            .is_none(),
        "non-target evidence must not restart the owner clock"
    );
    assert!(other_clock
        .due_update(
            wake_generation,
            pause_at + std::time::Duration::from_millis(1_000),
            super::EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS,
        )
        .is_some());
}

#[test]
fn installed_overlap_handoff_candidate_keeps_the_original_owner_deadline() {
    // Physical session 68ebb4b4: owner activity ended around 20.8s. A new
    // provider utterance plus a 0.055 local mismatch appeared at 21.6s, but
    // later Uncertain room speech kept renewing the old implementation until
    // 30.2s. The correlated candidate must preserve the original owner timer.
    let started = std::time::Instant::now();
    let owner = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("0".into()),
        target_speech_end_ms: Some(20_271),
        provider_audio_duration_ms: Some(20_500),
        audio_duration_ms: Some(20_800),
        local_speech_end_ms: Some(20_800),
        qualified_owner_speech_end_ms: Some(20_800),
        qualified_owner_activity_advanced: true,
        local_speaker_classification_kind: Some(crate::asr::volcengine::LocalSpeakerClassificationKind::Target),
        local_speaker_signal_quality_sufficient: Some(true),
        local_speaker_observation_end_ms: Some(20_800),
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: Some(20_800),
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(20_271),
        target_activity_advanced: true,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    let mut clock = super::SettledTargetEndpointClock::default();
    clock.note_visible_body_boundary(true, 77, started);
    let generation = clock
        .observe(&owner, true, started)
        .expect("last owner edge arms the one-second clock");

    let handoff_candidate = crate::asr::volcengine::TargetSpeakerUpdate {
        provider_audio_duration_ms: Some(22_100),
        audio_duration_ms: Some(22_100),
        local_speech_end_ms: Some(22_100),
        local_non_target_speech_end_ms: Some(22_100),
        target_activity_advanced: false,
        pending_unattributed_speech: true,
        pending_activity_advanced: false,
        ..owner
    };
    assert_eq!(
        clock.observe(
            &handoff_candidate,
            true,
            started + std::time::Duration::from_millis(900),
        ),
        None,
        "candidate room speech must not rearm from detection time",
    );
    assert_eq!(clock.generation, generation);
    assert!(clock
        .due_update(
            generation,
            started + std::time::Duration::from_millis(1_000),
            1_000,
        )
        .is_some());
}

#[test]
fn installed_session_1284_cloud_row_cannot_renew_enrolled_owner_endpoint() {
    // Session 1284: the local verifier last confirmed the owner at 11.9s.
    // Later Uncertain/low-score room speech was folded into the same cloud
    // speaker row through 15.322s. That cloud-only advance repeatedly rearmed
    // the wall clock and prevented the one-second owner-silence endpoint.
    let started = std::time::Instant::now();
    let owner = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("0".into()),
        target_speech_end_ms: Some(9_942),
        provider_audio_duration_ms: Some(12_200),
        audio_duration_ms: Some(12_300),
        local_speech_end_ms: Some(11_900),
        qualified_owner_speech_end_ms: Some(11_900),
        qualified_owner_activity_advanced: true,
        local_speaker_classification_kind: Some(crate::asr::volcengine::LocalSpeakerClassificationKind::Target),
        local_speaker_signal_quality_sufficient: Some(true),
        local_speaker_observation_end_ms: Some(11_900),
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: Some(11_900),
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(9_942),
        target_activity_advanced: true,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    assert_eq!(
        super::authoritative_owner_endpoint_boundary(&owner, owner.target_speech_end_ms),
        Some(11_900),
    );

    let mut clock = super::SettledTargetEndpointClock::default();
    clock.note_visible_body_boundary(true, 24, started);
    let generation = clock
        .observe(&owner, true, started)
        .expect("confirmed local owner arms endpoint");

    let cloud_only_room_speech = crate::asr::volcengine::TargetSpeakerUpdate {
        target_speech_end_ms: Some(15_322),
        provider_audio_duration_ms: Some(16_000),
        audio_duration_ms: Some(16_800),
        local_speech_end_ms: Some(16_100),
        stable_attributed_speech_end_ms: Some(15_322),
        target_activity_advanced: true,
        ..owner
    };
    assert_eq!(
        super::authoritative_owner_endpoint_boundary(
            &cloud_only_room_speech,
            cloud_only_room_speech.target_speech_end_ms,
        ),
        Some(11_900),
        "once local owner identity exists, cloud-only growth is not owner evidence",
    );
    assert_eq!(
        clock.observe(
            &cloud_only_room_speech,
            true,
            started + std::time::Duration::from_millis(800),
        ),
        None,
        "merged cloud speaker row must not restart the owner timer",
    );
    assert!(super::target_speaker_endpoint_due(&cloud_only_room_speech));
    assert!(clock
        .due_update(
            generation,
            started + std::time::Duration::from_millis(1_000),
            1_000,
        )
        .is_some());
}

#[test]
fn cloud_boundary_regression_rearms_local_owner_authority_instead_of_holding_forever() {
    let started = std::time::Instant::now();
    let initial = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("0".into()),
        target_speech_end_ms: Some(1_382),
        provider_audio_duration_ms: Some(1_900),
        audio_duration_ms: Some(2_000),
        local_speech_end_ms: Some(2_000),
        qualified_owner_speech_end_ms: Some(1_200),
        qualified_owner_activity_advanced: true,
        local_speaker_classification_kind: Some(crate::asr::volcengine::LocalSpeakerClassificationKind::Target),
        local_speaker_signal_quality_sufficient: Some(true),
        local_speaker_observation_end_ms: Some(1_200),
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: Some(1_200),
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(1_382),
        target_activity_advanced: true,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    let mut clock = super::SettledTargetEndpointClock::default();
    let first_generation = clock
        .observe(&initial, true, started)
        .expect("initial cloud/local owner boundary arms endpoint");

    let merged_room_speech = crate::asr::volcengine::TargetSpeakerUpdate {
        target_speech_end_ms: Some(5_152),
        provider_audio_duration_ms: Some(5_900),
        audio_duration_ms: Some(5_900),
        local_speech_end_ms: Some(5_600),
        stable_attributed_speech_end_ms: Some(5_152),
        target_activity_advanced: true,
        ..initial
    };
    let recovered_generation = clock
        .observe(
            &merged_room_speech,
            true,
            started + std::time::Duration::from_millis(900),
        )
        .expect("local authority recovery must replace the stale cloud boundary");
    assert_ne!(first_generation, recovered_generation);
    assert_eq!(clock.armed_target_end_ms, Some(1_200));
    assert!(clock
        .due_update(
            recovered_generation,
            started + std::time::Duration::from_millis(1_900),
            1_000,
        )
        .is_some());
}

#[test]
fn fresh_local_owner_recovery_still_rearms_after_cloud_only_growth_is_ignored() {
    let started = std::time::Instant::now();
    let first_owner = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("0".into()),
        target_speech_end_ms: Some(4_500),
        provider_audio_duration_ms: Some(5_000),
        audio_duration_ms: Some(5_100),
        local_speech_end_ms: Some(4_900),
        qualified_owner_speech_end_ms: Some(4_900),
        qualified_owner_activity_advanced: true,
        local_speaker_classification_kind: Some(crate::asr::volcengine::LocalSpeakerClassificationKind::Target),
        local_speaker_signal_quality_sufficient: Some(true),
        local_speaker_observation_end_ms: Some(4_900),
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: Some(4_900),
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(4_500),
        target_activity_advanced: true,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    let mut clock = super::SettledTargetEndpointClock::default();
    let first_generation = clock
        .observe(&first_owner, true, started)
        .expect("first owner boundary arms endpoint");

    let recovered_owner = crate::asr::volcengine::TargetSpeakerUpdate {
        target_speech_end_ms: Some(6_000),
        provider_audio_duration_ms: Some(6_100),
        audio_duration_ms: Some(6_200),
        local_speech_end_ms: Some(6_100),
        local_target_speech_end_ms: Some(6_100),
        stable_attributed_speech_end_ms: Some(6_000),
        ..first_owner
    };
    let recovered_generation = clock
        .observe(
            &recovered_owner,
            true,
            started + std::time::Duration::from_millis(700),
        )
        .expect("a fresh positive local Target must still rearm");
    assert_ne!(first_generation, recovered_generation);
    assert_eq!(clock.armed_target_end_ms, Some(6_100));
}

#[test]
fn late_two_pass_boundary_does_not_refresh_firmware_speech_timer() {
    let installed_session_36 = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("0".into()),
        target_speech_end_ms: Some(4_982),
        provider_audio_duration_ms: Some(8_700),
        audio_duration_ms: Some(8_800),
        local_speech_end_ms: Some(8_400),
        qualified_owner_speech_end_ms: Some(5_400),
        qualified_owner_activity_advanced: true,
        local_speaker_classification_kind: Some(crate::asr::volcengine::LocalSpeakerClassificationKind::Target),
        local_speaker_signal_quality_sufficient: Some(true),
        local_speaker_observation_end_ms: Some(5_400),
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: Some(5_400),
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(7_622),
        target_activity_advanced: true,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    assert!(!super::target_speaker_update_has_live_owner_activity(
        &installed_session_36
    ));
    assert!(super::target_speaker_endpoint_due_with_timeout(
        &installed_session_36,
        1_500
    ));

    let live_preview_growth = crate::asr::volcengine::TargetSpeakerUpdate {
        target_speech_end_ms: Some(8_450),
        local_target_speech_end_ms: Some(8_800),
        stable_attributed_speech_end_ms: Some(8_450),
        pending_activity_advanced: true,
        ..installed_session_36
    };
    assert!(super::target_speaker_update_has_live_owner_activity(
        &live_preview_growth
    ));
}

#[test]
fn confirmed_other_speech_does_not_count_as_live_owner_activity() {
    let other = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("0".into()),
        target_speech_end_ms: Some(4_000),
        provider_audio_duration_ms: Some(8_000),
        audio_duration_ms: Some(8_000),
        local_speech_end_ms: Some(8_000),
        qualified_owner_speech_end_ms: Some(4_000),
        qualified_owner_activity_advanced: true,
        local_speaker_classification_kind: Some(crate::asr::volcengine::LocalSpeakerClassificationKind::Target),
        local_speaker_signal_quality_sufficient: Some(true),
        local_speaker_observation_end_ms: Some(4_000),
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: Some(4_000),
        local_non_target_speech_end_ms: Some(8_000),
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(7_800),
        target_activity_advanced: true,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    assert!(!super::target_speaker_update_has_live_owner_activity(
        &other
    ));
    assert_eq!(
        super::target_speaker_fusion_state(&other),
        super::TargetSpeakerFusionState::ConfirmedOther
    );
}

#[test]
fn qualified_owner_activity_uncertain_tail_cannot_extend_endpoint_or_lease() {
    let started = std::time::Instant::now();
    let endpoint_timeout_ms = super::EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS;
    let owner_boundary_ms = endpoint_timeout_ms * 3;
    let raw_tail_ms = owner_boundary_ms
        + super::EMBEDDED_LOCAL_SPEECH_ALIGNMENT_SLACK_MS
        + 1;
    let late_cloud_boundary_ms = raw_tail_ms + 1;
    let initial = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("owner".into()),
        target_speech_end_ms: Some(owner_boundary_ms),
        provider_audio_duration_ms: Some(owner_boundary_ms),
        audio_duration_ms: Some(owner_boundary_ms),
        local_speech_end_ms: Some(owner_boundary_ms),
        qualified_owner_speech_end_ms: Some(owner_boundary_ms),
        qualified_owner_activity_advanced: true,
        local_speaker_classification_kind: Some(
            crate::asr::volcengine::LocalSpeakerClassificationKind::Target,
        ),
        local_speaker_signal_quality_sufficient: Some(true),
        local_speaker_observation_end_ms: Some(owner_boundary_ms),
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: Some(owner_boundary_ms),
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(owner_boundary_ms),
        target_activity_advanced: false,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    let mut clock = super::SettledTargetEndpointClock::default();
    let first_generation = clock
        .observe(&initial, true, started)
        .expect("qualified owner arms endpoint");

    // Raw audio advances before the classifier callback. It may pause the
    // existing stop candidate for a bounded identity decision, but it cannot
    // create a newer owner watermark or a firmware lease.
    let uncertain_tail = crate::asr::volcengine::TargetSpeakerUpdate {
        audio_duration_ms: Some(raw_tail_ms),
        local_speech_end_ms: Some(raw_tail_ms),
        qualified_owner_activity_advanced: false,
        local_speaker_classification_kind: Some(
            crate::asr::volcengine::LocalSpeakerClassificationKind::Uncertain,
        ),
        local_speaker_signal_quality_sufficient: Some(false),
        local_speaker_observation_end_ms: Some(owner_boundary_ms),
        local_tentative_owner_speech_end_ms: None,
        pending_unattributed_speech: true,
        target_activity_advanced: false,
        pending_activity_advanced: false,
        ..initial.clone()
    };
    assert_eq!(
        super::authoritative_owner_endpoint_boundary(
            &uncertain_tail,
            Some(late_cloud_boundary_ms),
        ),
        Some(owner_boundary_ms),
        "raw/uncertain audio cannot move the qualified owner boundary"
    );
    assert!(!clock.should_renew_firmware_endpoint_lease(&uncertain_tail, true));
    assert_eq!(clock.observe(&uncertain_tail, true, started + std::time::Duration::from_millis(200)), None);
    assert!(clock.armed_at.is_none());
    assert_eq!(clock.paused_armed_at, Some(started));
    assert_eq!(clock.paused_armed_target_end_ms, Some(owner_boundary_ms));

    // A late provider/preview revision carries a newer cloud edge but no new
    // qualified local observation. Restore the original candidate deadline;
    // do not restart it from callback arrival or renew firmware per packet.
    let late_provider_preview = crate::asr::volcengine::TargetSpeakerUpdate {
        target_speech_end_ms: Some(late_cloud_boundary_ms),
        provider_audio_duration_ms: Some(late_cloud_boundary_ms),
        stable_attributed_speech_end_ms: Some(late_cloud_boundary_ms),
        pending_unattributed_speech: false,
        target_activity_advanced: true,
        ..uncertain_tail.clone()
    };
    assert!(!super::authoritative_preview_growth_has_recent_owner_speech(
        &late_provider_preview,
        Some("owner preview"),
        Some("owner preview revised"),
    ));
    assert!(!clock.should_renew_firmware_endpoint_lease(&late_provider_preview, true));
    let restored_generation = clock
        .observe(
            &late_provider_preview,
            true,
            started + std::time::Duration::from_millis(600),
        )
        .expect("bounded uncertainty restores the original candidate");
    assert_ne!(restored_generation, first_generation);
    assert_eq!(clock.armed_target_end_ms, Some(owner_boundary_ms));
    assert_eq!(
        clock.armed_at,
        Some(started),
        "late provider/preview callbacks must not extend the wall-clock deadline"
    );

    // Repeating the same late snapshot is not a new activity edge.
    assert_eq!(
        clock.observe(
            &late_provider_preview,
            true,
            started + std::time::Duration::from_millis(900),
        ),
        None
    );
    assert!(!clock.should_renew_firmware_endpoint_lease(&late_provider_preview, true));

    let eventual_stop_at = started
        + std::time::Duration::from_millis(
            super::EMBEDDED_UNRESOLVED_LOCAL_SPEECH_MAX_HOLD_MS + endpoint_timeout_ms,
        );
    assert!(
        clock
            .due_update(restored_generation, eventual_stop_at, endpoint_timeout_ms)
            .is_some(),
        "the bounded uncertainty hold must eventually auto-stop"
    );
}

#[test]
fn installed_session_363_preview_growth_renews_firmware_before_one_second() {
    let update = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("0".into()),
        target_speech_end_ms: Some(9_662),
        provider_audio_duration_ms: Some(10_700),
        audio_duration_ms: Some(10_800),
        local_speech_end_ms: Some(10_200),
        qualified_owner_speech_end_ms: Some(9_700),
        qualified_owner_activity_advanced: true,
        local_speaker_classification_kind: Some(crate::asr::volcengine::LocalSpeakerClassificationKind::Target),
        local_speaker_signal_quality_sufficient: Some(true),
        local_speaker_observation_end_ms: Some(9_700),
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: Some(9_700),
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(9_662),
        target_activity_advanced: true,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };

    assert!(super::authoritative_preview_growth_has_recent_owner_speech(
        &update,
        Some("这句话仍然在连续增长到六十四个正文字符"),
        Some("这句话仍然在连续增长到六十七个正文字符而且没有停"),
    ));
    assert!(super::has_unresolved_recent_owner_speech(&update, 1_000));
    assert!(!super::target_speaker_endpoint_due_with_timeout(
        &update, 1_000,
    ));
}

#[test]
fn installed_lst_rec_054_owner_catch_up_lease_is_bounded_and_deduplicated() {
    let owner_provider_lag = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("0".into()),
        target_speech_end_ms: Some(4_500),
        provider_audio_duration_ms: Some(4_350),
        audio_duration_ms: Some(5_300),
        local_speech_end_ms: Some(5_300),
        qualified_owner_speech_end_ms: Some(4_500),
        qualified_owner_activity_advanced: true,
        local_speaker_classification_kind: Some(crate::asr::volcengine::LocalSpeakerClassificationKind::Target),
        local_speaker_signal_quality_sufficient: Some(true),
        local_speaker_observation_end_ms: Some(4_500),
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: Some(4_500),
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(4_500),
        target_activity_advanced: false,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    let mut clock = super::SettledTargetEndpointClock::default();
    assert!(clock.should_renew_firmware_endpoint_lease(&owner_provider_lag, true));
    assert!(
        !clock.should_renew_firmware_endpoint_lease(&owner_provider_lag, true),
        "repeated provider callbacks for one stale speech edge must not renew forever"
    );

    let next_owner_edge = crate::asr::volcengine::TargetSpeakerUpdate {
        audio_duration_ms: Some(5_700),
        local_speech_end_ms: Some(5_800),
        qualified_owner_speech_end_ms: Some(5_700),
        qualified_owner_activity_advanced: true,
        local_speaker_observation_end_ms: Some(5_700),
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: Some(5_700),
        ..owner_provider_lag.clone()
    };
    assert!(clock.should_renew_firmware_endpoint_lease(&next_owner_edge, true));

    let confirmed_other = crate::asr::volcengine::TargetSpeakerUpdate {
        audio_duration_ms: Some(6_000),
        local_speech_end_ms: Some(6_000),
        local_non_target_speech_end_ms: Some(6_000),
        ..owner_provider_lag.clone()
    };
    assert!(!clock.should_renew_firmware_endpoint_lease(&confirmed_other, true));

    let uncertainty_budget_exhausted = crate::asr::volcengine::TargetSpeakerUpdate {
        audio_duration_ms: Some(6_600),
        local_speech_end_ms: Some(6_600),
        ..owner_provider_lag
    };
    assert!(!clock.should_renew_firmware_endpoint_lease(&uncertainty_budget_exhausted, true,));
}

#[test]
fn preview_growth_firmware_refresh_rejects_punctuation_stale_and_other_speaker() {
    let base = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("0".into()),
        target_speech_end_ms: Some(9_662),
        provider_audio_duration_ms: Some(10_700),
        audio_duration_ms: Some(10_800),
        local_speech_end_ms: Some(10_200),
        qualified_owner_speech_end_ms: Some(9_700),
        qualified_owner_activity_advanced: true,
        local_speaker_classification_kind: Some(crate::asr::volcengine::LocalSpeakerClassificationKind::Target),
        local_speaker_signal_quality_sufficient: Some(true),
        local_speaker_observation_end_ms: Some(9_700),
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: Some(9_700),
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(9_662),
        target_activity_advanced: true,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    assert!(
        !super::authoritative_preview_growth_has_recent_owner_speech(
            &base,
            Some("本人说完了"),
            Some("本人说完了。"),
        )
    );

    let stale = crate::asr::volcengine::TargetSpeakerUpdate {
        local_speech_end_ms: Some(9_700),
        ..base.clone()
    };
    assert!(
        !super::authoritative_preview_growth_has_recent_owner_speech(
            &stale,
            Some("本人说了第一句"),
            Some("本人说了第一句但这是迟到修订"),
        )
    );

    let confirmed_other = crate::asr::volcengine::TargetSpeakerUpdate {
        local_non_target_speech_end_ms: Some(10_200),
        ..base.clone()
    };
    assert!(
        !super::authoritative_preview_growth_has_recent_owner_speech(
            &confirmed_other,
            Some("本人说了第一句"),
            Some("本人说了第一句旁人正在继续"),
        )
    );

    let provider_other = crate::asr::volcengine::TargetSpeakerUpdate {
        stable_attributed_speech_end_ms: Some(10_300),
        ..base
    };
    assert!(
        !super::authoritative_preview_growth_has_recent_owner_speech(
            &provider_other,
            Some("本人说了第一句"),
            Some("本人说了第一句房间里还有声音"),
        )
    );
}

#[test]
fn firmware_speech_refresh_coalesces_latest_instead_of_dropping_it() {
    let mut queue = super::LatestSpeechActivityQueue::default();

    assert!(queue.enqueue(1));
    assert!(!queue.enqueue(2));
    assert!(!queue.enqueue(3));
    assert_eq!(queue.take_pending_or_finish(), Some(3));
    assert_eq!(queue.take_pending_or_finish(), None);

    assert!(queue.enqueue(4));
    assert_eq!(queue.take_pending_or_finish(), Some(4));
    assert_eq!(queue.take_pending_or_finish(), None);

    assert!(
        super::EMBEDDED_ASR_SPEECH_ACTIVITY_MAX_QUEUE_AGE
            + super::EMBEDDED_ASR_SPEECH_ACTIVITY_TIMEOUT
            < std::time::Duration::from_millis(super::EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS),
        "a coalesced refresh must either arrive before the one-second endpoint or be discarded"
    );
}

#[test]
fn explicit_wake_diagnostic_sequence_stops_at_retention_limit() {
    assert_eq!(
        super::next_wake_diagnostic_capture_count(false, super::WAKE_DIAGNOSTIC_MAX_CANDIDATES),
        None,
        "an explicit operator-managed capture session must remain bounded"
    );
}

#[test]
fn production_default_resolves_zero_wake_diagnostic_targets_for_one_hundred_candidates() {
    for _ in 0..100 {
        assert!(super::explicit_wake_diagnostic_directory(None).is_none());
    }
    assert!(super::explicit_wake_diagnostic_directory(Some("   ".into())).is_none());
    assert_eq!(
        super::explicit_wake_diagnostic_directory(Some("D:\\listener-wake-diag".into())),
        Some(std::path::PathBuf::from("D:\\listener-wake-diag"))
    );
}

#[test]
fn local_shadow_recovers_bounded_middle_and_tail_omissions_without_rewriting_cloud_text() {
    let cloud = "我现在准备测试这个录音系统的完整效果，看看最后结尾是否正常。";
    let local = "我现在认真准备测试这个录音系统的完整效果看看最后完整结尾是否正常";
    assert_eq!(
        super::recover_local_shadow_omissions(cloud, local).as_deref(),
        Some("我现在认真准备测试这个录音系统的完整效果，看看最后完整结尾是否正常。")
    );
}

#[test]
fn local_shadow_rejects_single_character_model_insertions() {
    assert_eq!(
        super::recover_local_shadow_omissions(
            "我们要做一个说话人识别的测试。",
            "我们要做一个说话人力识别的测试",
        ),
        None,
        "the observed Paraformer one-character insertion must not alter cloud text"
    );
}

#[test]
fn local_shadow_rejects_observed_public_overlap_interferer_tail() {
    assert_eq!(
        super::recover_local_shadow_omissions(
            "我们要做一个说话人识别的测试。",
            "我们要做一个说话人识别的测试年度演讲",
        ),
        None,
        "the real local overlap decode must not restore the interfering speaker tail"
    );
}

#[test]
fn local_shadow_rejects_rewrites_and_large_other_speaker_gaps() {
    assert_eq!(
        super::recover_local_shadow_omissions("今天检查录音是否完整。", "今天检测录音是否完整",),
        None,
        "a local substitution is not omission evidence"
    );
    assert_eq!(
        super::recover_local_shadow_omissions(
            "本人第一句然后本人第二句。",
            "本人第一句旁边的人连续说了很长一段无关内容然后本人第二句",
        ),
        None,
        "a long overlap gap can be another speaker and must never be restored"
    );
}

fn product_final_candidates(primary: &str) -> super::ProductFinalCandidates {
    super::ProductFinalCandidates {
        provider_primary: crate::asr::RawTranscript {
            text: primary.into(),
            duration_ms: 7_462,
        },
        separated_owner: None,
        retained_audio_replay: None,
        debug_override: None,
        partial_preview: None,
        local_shadow: None,
        local_shadow_owner_end_aligned: false,
        target_filter_required: false,
        primary_speaker_filtered_certified: false,
    }
}

#[test]
fn product_final_long_other_uses_authoritative_owner_preview_over_visible_tail() {
    // Captured 899c3910: a later authoritative 80-character owner correction
    // left a longer visual tail in the capsule's display high-water mark.
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    let owner = "今天的工作顺序是先完成设备配对和权限检查，然后测试一段大约20秒的自然中文语音，最后在提交前确认预览已经逐步收敛到最终文本。任何失败都要保留时间点和可追溯原因。";
    let foreign_tail =
        format!("{owner}旁边的人正在讨论今晚吃什么，这些话不应该添加到刚才的正文里面");
    {
        let mut preview = coordinator.inner.embedded_audio_preview.lock();
        preview.begin_session(session_id);
        preview.observe_authoritative(session_id, owner, true);
        preview.observe_provisional(session_id, &foreign_tail);
        preview.observe_authoritative(session_id, owner, true);
        assert_eq!(
            preview.visible(session_id).as_deref(),
            Some(foreign_tail.as_str())
        );
    }

    let partial =
        current_embedded_audio_final_preview_candidate(&coordinator.inner, session_id, 7_462)
            .expect("authoritative owner preview");
    assert_eq!(partial.text, owner);

    let mut candidates = product_final_candidates(owner);
    candidates.target_filter_required = true;
    candidates.partial_preview = Some(partial);
    let decision = super::arbitrate_product_final_transcript(candidates, &[], false);
    assert_eq!(decision.transcript.text, owner);
}

#[test]
fn product_final_preview_filters_automatic_wake_before_selection() {
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    arm_automatic_wake_text_guard(&coordinator.inner, session_id, "开始录音".into(), 1_200);
    acknowledge_automatic_wake_capsule_visible(&coordinator.inner, session_id);
    {
        let mut preview = coordinator.inner.embedded_audio_preview.lock();
        preview.begin_session(session_id);
        preview.observe_authoritative(session_id, "开始录音今天继续测试", true);
        preview.observe_provisional(session_id, "开始录音今天继续测试旁人尾巴");
    }

    let partial =
        current_embedded_audio_final_preview_candidate(&coordinator.inner, session_id, 2_500)
            .expect("wake-filtered authoritative preview");
    assert_eq!(partial.text, "今天继续测试");
    let mut candidates = product_final_candidates("");
    candidates.partial_preview = Some(partial);
    assert_eq!(
        super::arbitrate_product_final_transcript(candidates, &[], false)
            .transcript
            .text,
        "今天继续测试"
    );
}

#[test]
fn product_final_separated_owner_keeps_punctuated_provider_rendering() {
    // r23 2026-09-18 921fe510（TTS 旁人干扰轮，用户反馈"吞标点"）：干扰使终稿走
    // separated_owner 通道。ASR 侧已把分离轨 final 按归属边界映射回 provider 说话
    // 人过滤原文的带标点切片；这里验证产品边界（dictation.rs 的唤醒前缀剥离 +
    // arbitrate_product_final_transcript）不会再次弄丢标点或"1秒"的数字写法，
    // 且 provider 原文里的旁人尾巴（"我看一下这个呢。是100分。……"）仍被排除。
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    arm_automatic_wake_text_guard(&coordinator.inner, session_id, "开始录音".into(), 1_200);
    acknowledge_automatic_wake_capsule_visible(&coordinator.inner, session_id);

    // ASR trace provider_raw_result final（唤醒词 + 正文 + 旁人尾巴，verbatim）。
    let provider_primary_text = "开始录音，我现在做端点复测。第一部分先完整保留，中间自然停顿大约1秒，然后继续第二部分，最后这句话也必须要完整保留。我看一下这个呢。是100分。我看一下这个呢。";
    // 分离轨按边界恢复后的 separated_owner 候选 = provider 说话人过滤原文切片。
    let separated_owner_text = "开始录音，我现在做端点复测。第一部分先完整保留，中间自然停顿大约1秒，然后继续第二部分，最后这句话也必须要完整保留。";

    let mut candidates = product_final_candidates(&super::filter_automatic_wake_text(
        &coordinator.inner,
        session_id,
        provider_primary_text,
        false,
    ));
    candidates.separated_owner = Some(crate::asr::RawTranscript {
        text: super::filter_automatic_wake_text(&coordinator.inner, session_id, separated_owner_text, false),
        duration_ms: 15_286,
    });
    candidates.target_filter_required = true;
    // r36：此处主轨是未过滤的 provider_raw 全文（带旁人尾，无认证）——分离稿
    // 是它的紧凑子序列（只少尾巴=在做排除），子序列豁免必须仍然生效。
    candidates.primary_speaker_filtered_certified = false;
    let decision = super::arbitrate_product_final_transcript(candidates, &[], false);
    assert_eq!(
        decision.transcript.text,
        "我现在做端点复测。第一部分先完整保留，中间自然停顿大约1秒，然后继续第二部分，最后这句话也必须要完整保留。",
        "separated owner final keeps the punctuated provider rendering and the 1秒 digit form"
    );
    assert_eq!(
        decision.authority,
        crate::speech_decision_kernel::ProductFinalAuthority::SeparatedOwner
    );
}

#[test]
fn product_final_authoritative_preview_survives_empty_provider() {
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    let owner = "This accepted owner preview contains the complete sentence.";
    {
        let mut preview = coordinator.inner.embedded_audio_preview.lock();
        preview.begin_session(session_id);
        preview.observe_authoritative(session_id, owner, true);
    }
    let partial =
        current_embedded_audio_final_preview_candidate(&coordinator.inner, session_id, 900)
            .expect("authoritative preview");
    let mut candidates = product_final_candidates("");
    candidates.partial_preview = Some(partial);
    assert_eq!(
        super::arbitrate_product_final_transcript(candidates, &[], false)
            .transcript
            .text,
        owner
    );
}

#[test]
fn product_final_filtering_invalidates_pre_filter_preview_before_empty_arbitration() {
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    {
        let mut preview = coordinator.inner.embedded_audio_preview.lock();
        preview.begin_session(session_id);
        preview.observe_authoritative(session_id, "主人正文", true);
        preview.observe_provisional(session_id, "主人正文旁人尾巴");
    }
    assert!(invalidate_embedded_audio_authoritative_preview(
        &coordinator.inner,
        session_id,
        "target_filter_required",
    ));

    assert!(
        current_embedded_audio_final_preview_candidate(&coordinator.inner, session_id, 900)
            .is_none(),
        "a preview observed before owner filtering must not become an empty-provider final"
    );
    let mut candidates = product_final_candidates("");
    candidates.target_filter_required = true;
    let decision = super::arbitrate_product_final_transcript(candidates, &[], false);
    assert!(decision.transcript.text.is_empty());
}

#[test]
fn product_final_provisional_only_preview_is_never_a_final_candidate() {
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    {
        let mut preview = coordinator.inner.embedded_audio_preview.lock();
        preview.begin_session(session_id);
        preview.observe_provisional(session_id, "仅有暂态旁路文字");
    }
    assert!(
        current_embedded_audio_final_preview_candidate(&coordinator.inner, session_id, 900)
            .is_none()
    );
    let candidates = product_final_candidates("");
    let decision = super::arbitrate_product_final_transcript(candidates, &[], false);
    assert!(decision.transcript.text.is_empty());
}

#[test]
fn product_final_preview_rejects_stale_session_identity() {
    let coordinator = Coordinator::new();
    let stale_session = new_session_id();
    let current_session = new_session_id();
    {
        let mut preview = coordinator.inner.embedded_audio_preview.lock();
        preview.begin_session(stale_session);
        preview.observe_authoritative(stale_session, "stale owner", true);
        preview.begin_session(current_session);
        preview.observe_authoritative(current_session, "current owner", true);
    }
    assert!(
        current_embedded_audio_final_preview_candidate(&coordinator.inner, stale_session, 900)
            .is_none()
    );
    assert_eq!(
        current_embedded_audio_final_preview_candidate(&coordinator.inner, current_session, 900)
            .expect("current session preview")
            .text,
        "current owner"
    );
}

#[test]
fn product_final_does_not_replace_provider_body_with_longer_preview() {
    let mut candidates = product_final_candidates("今天天气");
    candidates.partial_preview = Some(crate::asr::RawTranscript {
        text: "今天天气很好".into(),
        duration_ms: 7_462,
    });
    let decision = super::arbitrate_product_final_transcript(candidates, &[], false);
    assert_eq!(
        decision.transcript.text, "今天天气",
        "a longer preview cannot override a non-empty provider body"
    );
}

#[test]
fn r24_degraded_separated_owner_never_overrides_covering_speaker_filtered_primary() {
    // r24 2026-09-18 session 999a5b67（TTS 旁人干扰轮）：第一仲裁已封存正确
    // 52 字 speaker_filtered 终稿（无旁人尾），但分离音轨二次解码受提取伪影
    // 拖累只剩 24 字劣化稿（"第二步"），产品仲裁无条件偏向 separated_owner
    // → 用户判"吞字"。说出内容被主终稿覆盖（≥）的分离稿必须让位；分离稿
    // 听到更多内容时维持分离优先（下一测试）。
    let mut candidates = product_final_candidates(
        "我现在做端点复测，第一部分先完整保留，中间自然大约停顿一秒，然后继续第二部分，最后这句话也必须要完整保留。",
    );
    candidates.target_filter_required = true;
    candidates.primary_speaker_filtered_certified = true;
    candidates.separated_owner = Some(crate::asr::RawTranscript {
        text: "我现在做端点复测。大约停顿一秒，然后继续第二步。".into(),
        duration_ms: 7_462,
    });
    let decision = super::arbitrate_product_final_transcript(candidates, &[], false);
    assert_eq!(
        decision.authority,
        crate::speech_decision_kernel::ProductFinalAuthority::ProviderPrimary
    );
    assert_eq!(
        decision.transcript.text,
        "我现在做端点复测，第一部分先完整保留，中间自然大约停顿一秒，然后继续第二部分，最后这句话也必须要完整保留。"
    );
}

#[test]
fn r36_certified_primary_demotes_truncated_separated_prefix() {
    // r36 2026-09-18 晚（tij 实机，用户"吞字"）：主轨 speaker_filtered 封存
    // 51 字完好终稿，分离稿被截断成主轨**前缀**（24 原文/20 有效字）。文本层
    // 面"前缀"与"排除旁人尾后剩正文"不可区分，r24 的子序列豁免把截断稿当
    // 排除工作放行 → 只交付 20 字。认证位（speaker_filtered 已切尾）使覆盖
    // 即胜出：主终稿本身已是排除产物，分离稿更短只能是丢正文，必须降权。
    let primary = "我现在做端点复测。第一部分先完整保留，中间自然停顿大约1秒，然后继续第二部分，最后这句话也必须要完整保留。";
    let mut candidates = product_final_candidates(primary);
    candidates.target_filter_required = true;
    candidates.primary_speaker_filtered_certified = true;
    // 分离稿 = 主轨前缀（compact 子序列），r24 豁免会放行的形态。
    candidates.separated_owner = Some(crate::asr::RawTranscript {
        text: "我现在做端点复测。第一部分先完整保留，中间自然停顿大约1秒。".into(),
        duration_ms: 7_462,
    });
    let decision = super::arbitrate_product_final_transcript(candidates, &[], false);
    assert_eq!(
        decision.authority,
        crate::speech_decision_kernel::ProductFinalAuthority::ProviderPrimary,
        "certified speaker-filtered primary must beat a truncated separated prefix"
    );
    assert_eq!(decision.transcript.text, primary);
}

#[test]
fn r24_separated_owner_still_wins_when_it_hears_more_than_primary() {
    // 分离轨听到更多内容（主终稿缺字、分离稿补全）时，既有分离优先语义不变；
    // 认证位不改变这一点——认证只裁决"主终稿覆盖分离稿"的形态。
    let mut candidates = product_final_candidates("我现在做端点复测。");
    candidates.target_filter_required = true;
    candidates.primary_speaker_filtered_certified = true;
    candidates.separated_owner = Some(crate::asr::RawTranscript {
        text: "我现在做端点复测。第一部分先完整保留。".into(),
        duration_ms: 7_462,
    });
    let decision = super::arbitrate_product_final_transcript(candidates, &[], false);
    assert_eq!(
        decision.authority,
        crate::speech_decision_kernel::ProductFinalAuthority::SeparatedOwner
    );
    assert_eq!(decision.transcript.text, "我现在做端点复测。第一部分先完整保留。");
}

#[test]
fn product_final_filler_setting_controls_punctuation_cleanup() {
    let text = "这样可以，嗯？";
    assert_eq!(
        super::arbitrate_product_final_transcript(product_final_candidates(text), &[], false)
            .transcript
            .text,
        text
    );
    assert_eq!(
        super::arbitrate_product_final_transcript(product_final_candidates(text), &[], true)
            .transcript
            .text,
        "这样可以？"
    );
}

#[test]
fn product_final_does_not_restore_unverified_preview_tail_under_interference() {
    let mut candidates = product_final_candidates("今天天气");
    candidates.target_filter_required = true;
    candidates.partial_preview = Some(crate::asr::RawTranscript {
        text: "今天天气旁边的人还在说话".into(),
        duration_ms: 7_462,
    });
    let decision = super::arbitrate_product_final_transcript(candidates, &[], false);
    assert_eq!(decision.transcript.text, "今天天气");
}

#[test]
fn target_speaker_endpoint_product_final_chooses_separated_owner_once_under_interference() {
    let mut candidates = product_final_candidates("");
    candidates.target_filter_required = true;
    candidates.separated_owner = Some(crate::asr::RawTranscript {
        text: "主人第一句主人第二句".into(),
        duration_ms: 7_462,
    });
    candidates.partial_preview = Some(crate::asr::RawTranscript {
        text: "主人第一句旁边的人无关内容主人第二句".into(),
        duration_ms: 7_462,
    });
    candidates.local_shadow = Some("主人第一句旁边的人无关内容主人第二句".into());
    candidates.local_shadow_owner_end_aligned = true;

    let decision = super::arbitrate_product_final_transcript(candidates, &[], false);
    assert_eq!(decision.transcript.text, "主人第一句主人第二句");
    assert_eq!(
        decision.authority,
        crate::speech_decision_kernel::ProductFinalAuthority::SeparatedOwner
    );
    assert!(!decision.local_shadow_recovered);
}

#[test]
fn target_speaker_endpoint_product_final_never_restores_unverified_text_when_filter_is_required() {
    let mut candidates = product_final_candidates("");
    candidates.target_filter_required = true;
    candidates.retained_audio_replay = Some(crate::asr::RawTranscript {
        text: "重放混入旁人内容".into(),
        duration_ms: 7_462,
    });
    candidates.partial_preview = Some(crate::asr::RawTranscript {
        text: "预览混入旁人内容".into(),
        duration_ms: 7_462,
    });
    candidates.local_shadow = Some("本地模型混入旁人内容".into());
    candidates.local_shadow_owner_end_aligned = true;

    let decision = super::arbitrate_product_final_transcript(candidates, &[], false);
    assert_eq!(
        decision.transcript.text, "",
        "unverified preview, replay, and shadow must not enter final text under isolation"
    );
    assert_eq!(
        decision.authority,
        crate::speech_decision_kernel::ProductFinalAuthority::Empty
    );
}

#[test]
fn target_speaker_endpoint_product_final_recovers_cloud_omission_only_without_interference() {
    let mut candidates =
        product_final_candidates("我现在准备测试这个录音系统的完整效果，看看最后结尾是否正常。");
    candidates.local_shadow =
        Some("我现在认真准备测试这个录音系统的完整效果看看最后完整结尾是否正常".into());
    candidates.local_shadow_owner_end_aligned = true;

    let decision = super::arbitrate_product_final_transcript(candidates, &[], false);
    assert_eq!(
        decision.transcript.text,
        "我现在认真准备测试这个录音系统的完整效果，看看最后完整结尾是否正常。"
    );
    assert!(decision.local_shadow_recovered);
}

#[test]
fn target_speaker_endpoint_product_final_uses_replay_before_preview_for_clean_empty_primary() {
    let mut candidates = product_final_candidates("");
    candidates.retained_audio_replay = Some(crate::asr::RawTranscript {
        text: "重放恢复的主人正文".into(),
        duration_ms: 7_462,
    });
    candidates.partial_preview = Some(crate::asr::RawTranscript {
        text: "较旧的预览正文".into(),
        duration_ms: 7_462,
    });

    let decision = super::arbitrate_product_final_transcript(candidates, &[], false);
    assert_eq!(decision.transcript.text, "重放恢复的主人正文");
    assert_eq!(
        decision.authority,
        crate::speech_decision_kernel::ProductFinalAuthority::RetainedAudioReplay
    );
}

#[test]
fn candidate_slice_ranges_become_unknown_on_missing_or_overflowed_base() {
    assert_eq!(
        super::candidate_slice_range(Some(100), 12, 8),
        Some(crate::observability::CandidateRange {
            start: 112,
            end: 120
        })
    );
    assert!(super::candidate_slice_range(None, 12, 8).is_none());
    assert!(super::candidate_slice_range(Some(u64::MAX - 3), 0, 8).is_none());
}

#[test]
fn candidate_release_paths_keep_original_slices_and_actual_kws_feed_length() {
    let source = include_str!("dictation_embedded_stream.rs");
    assert!(source.contains("let actual_feed_bytes = pcm_to_feed.len();"));
    assert!(source.contains("candidate.kws_fed_bytes = incremental_feed_start;"));
    assert!(source.contains("let terminal_feed_bytes = remaining_pcm.len();"));
    assert!(source.contains("CandidateFactKind::KwsFeedUnknown"));
    assert!(source.matches("CandidateFactKind::KwsFeedUnknown").count() >= 2);
    assert!(source.contains("Some(\"terminal_accept_pcm_feed\")"));
    assert!(source.contains("Some(\"streaming_kws_task_join_unknown\")"));
    assert!(source.contains("let candidate_source_observation = source_observation_is_explicit"));

    for function in [
        "async fn finish_buffered_speaker_candidate(",
        "async fn try_release_automatic_candidate(",
    ] {
        let function_start = source.find(function).expect("candidate function exists");
        let body = &source[function_start..];
        let release_start = body
            .find("let destination_session_id = session.session_id.to_string();")
            .expect("release ledger exists");
        let release_body = &body[release_start..];
        let release_end = release_body
            .find("session.candidate_fact_ledger = Some")
            .expect("candidate ledger is transferred");
        let release_body = &release_body[..release_end];
        assert!(release_body.contains("CandidateFactKind::ReleaseAttempted"));
        assert!(release_body.contains("CandidateFactKind::ReleaseAccepted"));
        assert!(release_body.contains("CandidateFactKind::ReleaseOutcomeUnknown"));
        assert!(release_body.contains("coordinator_partial_acceptance_range_unknown"));
        assert!(release_body.contains("candidate_slice_range("));
        assert!(release_body.contains("accepted_bytes_before = session.streamed_pcm_bytes"));
        assert!(!release_body.contains("kws_fed_bytes"));
        assert!(!release_body.contains("candidate.pcm[release_offset..release_end].to_vec()"));
    }
}

#[test]
fn preserved_candidate_ledgers_keep_distinct_candidates_and_trace_capacity_drop() {
    let mut streaming = EmbeddedStreamingDictation::default();
    let mut first =
        embedded_audio_test_session(uuid::Uuid::nil(), Arc::new(CapturingConsumer::default()));
    first.candidate_id = Some(101);
    first.candidate_fact_ledger = Some(crate::observability::CandidateFactLedger::default());
    streaming.preserve_session_candidate_fact_ledger(&mut first);

    let mut second =
        embedded_audio_test_session(uuid::Uuid::nil(), Arc::new(CapturingConsumer::default()));
    second.candidate_id = Some(102);
    second.candidate_fact_ledger = Some(crate::observability::CandidateFactLedger::default());
    streaming.preserve_session_candidate_fact_ledger(&mut second);
    assert!(first.candidate_fact_ledger.is_none());
    assert!(second.candidate_fact_ledger.is_none());
    assert_eq!(
        streaming.preserved_candidate_ledger_ids_for_test(),
        vec![101, 102]
    );

    for candidate_id in 0..PRESERVED_CANDIDATE_LEDGER_CAPACITY {
        streaming.preserve_candidate_fact_ledger_with_id(
            1_000 + candidate_id as u64,
            crate::observability::CandidateFactLedger::default(),
        );
    }
    let (ids, drop_count, dropped_ids, incomplete) =
        streaming.preserved_candidate_ledger_state_for_test();
    assert_eq!(ids.len(), PRESERVED_CANDIDATE_LEDGER_CAPACITY);
    assert_eq!(ids.first().copied(), Some(1_000));
    assert_eq!(
        ids.last().copied(),
        Some(1_000 + PRESERVED_CANDIDATE_LEDGER_CAPACITY as u64 - 1)
    );
    assert_eq!(drop_count, 2);
    assert_eq!(dropped_ids, vec![101, 102]);
    assert!(incomplete);
}

#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
#[test]
fn extracted_wake_requires_exact_phrase_and_independent_owner_source() {
    assert!(super::target_extracted_wake_can_activate(true, true, false));
    assert!(super::target_extracted_wake_can_activate(true, false, true));
    assert!(!super::target_extracted_wake_can_activate(
        true, false, false
    ));
    assert!(!super::target_extracted_wake_can_activate(
        false, true, true
    ));
}

#[test]
fn accepted_wake_capture_request_ids_seed_above_any_prior_process_watermark() {
    // r46e: the firmware's boot-scoped ensure watermark (never reset on the
    // reconnect path) rejected a fresh Type process's first `request_id=1` as
    // stale, the wake-capture lease renewal was dropped, and the device
    // auto-stopped its hidden window mid-body.  The counter must therefore be
    // seeded from the wall clock so a restarted process allocates ids above
    // anything a previous process on the same device boot could have sent.
    let unix_seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let first = super::next_accepted_wake_capture_request_id();
    let second = super::next_accepted_wake_capture_request_id();
    assert!(
        first >= unix_seconds.saturating_sub(60).min(0x7FFF_FFFF as u64) as u32,
        "first id {first} must be seeded from wall clock (unix {unix_seconds})"
    );
    assert!(second == first + 1, "ids increment: {first} -> {second}");
}

#[test]
fn r46f_fresh_body_start_blocks_inherited_wake_era_stop() {
    // r46f (2026-09-19 10:23): the user's natural post-wake pause (~1.6 s)
    // let the 1 s inactive deadline — armed by the wake phrase itself — fire
    // 40 ms BEFORE the first body preview became visible. The dispatch guard
    // must treat a just-latched body as live: block the inherited stop and let
    // the reading's own activity re-arm the clock. Once the grace window has
    // passed without follow-up evidence, the ordinary auto-end resumes.
    let quiet_no_growth = crate::asr::volcengine::TargetSpeakerUpdate {
        speaker_id: Some("0".into()),
        target_speech_end_ms: Some(1_000),
        provider_audio_duration_ms: Some(4_000),
        audio_duration_ms: Some(4_000),
        local_speech_end_ms: Some(1_000),
        qualified_owner_speech_end_ms: Some(1_000),
        qualified_owner_activity_advanced: false,
        local_speaker_classification_kind: Some(crate::asr::volcengine::LocalSpeakerClassificationKind::Target),
        local_speaker_signal_quality_sufficient: Some(true),
        local_speaker_observation_end_ms: Some(1_000),
        local_tentative_owner_speech_end_ms: None,
        local_target_speech_end_ms: Some(1_000),
        local_non_target_speech_end_ms: None,
        local_speaker_tracking_enabled: true,
        stable_attributed_speech_end_ms: Some(1_000),
        target_activity_advanced: false,
        pending_unattributed_speech: false,
        pending_activity_advanced: false,
        speaker_info_present: true,
    };
    assert!(
        super::owner_endpoint_stop_blocked_by_live_owner(
            super::TargetSpeakerFusionState::Quiet,
            &quiet_no_growth,
            true,
            false,
            true
        ),
        "a body that just started must not be cut by the wake-era deadline"
    );
    assert!(
        !super::owner_endpoint_stop_blocked_by_live_owner(
            super::TargetSpeakerFusionState::Quiet,
            &quiet_no_growth,
            true,
            false,
            false
        ),
        "once the body-start grace expires, Quiet without growth still auto-ends"
    );
    assert!(
        !super::owner_endpoint_stop_blocked_by_live_owner(
            super::TargetSpeakerFusionState::ConfirmedOther,
            &quiet_no_growth,
            true,
            false,
            true
        ),
        "the grace does not weaken the settled ConfirmedOther contract"
    );
}

#[tokio::test]
async fn r46f_expired_ensure_keeps_session_with_live_body_text() {
    // r46f: the continuation marker was still in flight when the 3 s ensure
    // deadline expired; the hard abort cancelled a session whose preview was
    // actively growing and sealed 0 of 23 chars. An expired ensure must only
    // abort when no body evidence exists — with live body text it drops the
    // handshake and leaves the product session to the ordinary endpoint.
    let coordinator = Coordinator::new();
    let session_id = new_session_id();
    {
        let mut state = coordinator.inner.state.lock();
        state.session_id = session_id;
        state.phase = SessionPhase::Listening;
        state.cancelled = false;
    }
    register_embedded_ble_cancel_flag(&coordinator.inner, &Arc::new(AtomicBool::new(false)));
    let consumer = Arc::new(CountingConsumer::default());
    let consumer_for_session: Arc<dyn crate::recorder::AudioConsumer> = consumer.clone();
    let mut streaming = EmbeddedStreamingDictation::background_listener();
    streaming.embedded_session_id = Some(93);
    streaming.session = Some(embedded_audio_test_session(
        session_id,
        consumer_for_session,
    ));
    // Body latch flipped just now: the guard says this session has live body.
    super::arm_automatic_wake_text_guard(
        &coordinator.inner,
        session_id,
        "开始录音".to_string(),
        900,
    );
    {
        let mut guard = coordinator
            .inner
            .embedded_audio_automatic_wake_guard
            .lock();
        let guard = guard.as_mut().expect("wake guard installed");
        guard.body_started = true;
        guard.body_started_at = Some(Instant::now());
    }
    let past = Instant::now() - Duration::from_secs(30);
    streaming.accepted_wake_capture_ensure = Some(super::AcceptedWakeCaptureEnsure {
        request_id: 1789784622,
        previous_segment_id: 93,
        requested_at: past,
        deadline_at: past + super::EMBEDDED_ACCEPTED_WAKE_CAPTURE_REPLACEMENT_TIMEOUT,
        replacement_wait_started_at: None,
        confirmed_segment_id: None,
    });

    let aborted = streaming.expire_accepted_wake_capture_if_due(&coordinator.inner);

    assert!(
        !aborted,
        "live body session must not be aborted by an expired ensure"
    );
    assert!(
        streaming.accepted_wake_capture_ensure.is_none(),
        "the dead handshake is dropped either way"
    );
    assert!(
        streaming.session.is_some(),
        "the product session survives to its normal endpoint"
    );
}

#[test]
fn pause_early_final_remainder_splits_on_stability_key_prefix() {
    use super::pause_early_final_remainder;
    let key = |text: &str| super::embedded_audio_partial_preview_stability_key(text);

    // Growth: only the tail needs the second insertion.
    let delivered = "现在是什么问题？你帮我看一下。";
    let final_text = "现在是什么问题？你帮我看一下。然后你要不要搞一个什么测试窗口？";
    let remainder =
        pause_early_final_remainder(final_text, delivered, &key(delivered)).expect("prefix covers final");
    assert_eq!(remainder, "然后你要不要搞一个什么测试窗口？");

    // A punctuation mark that arrived after a word-only paste belongs to the
    // still-undelivered boundary. It must survive the stability-key split.
    assert_eq!(
        pause_early_final_remainder("你好，继续说。", "你好", &key("你好")),
        Some("，继续说。".to_string())
    );
    assert_eq!(
        pause_early_final_remainder("你好。", "你好", &key("你好")),
        Some("。".to_string())
    );
    assert_eq!(
        pause_early_final_remainder("你好，继续说。", "你好，", &key("你好，")),
        Some("继续说。".to_string())
    );
    assert_eq!(
        pause_early_final_remainder("你好，“继续说”。", "你好，", &key("你好，")),
        Some("“继续说”。".to_string()),
        "deduplicating a comma must keep the next opening quote"
    );

    // Cloud punctuation revision inside the prefix must not block the split.
    let delivered_revised = "现在是什么问题，你帮我看一下。";
    let remainder = pause_early_final_remainder(final_text, delivered_revised, &key(delivered_revised))
        .expect("punctuation-insensitive prefix still matches");
    assert_eq!(remainder, "然后你要不要搞一个什么测试窗口？");

    // Exact coverage (including a punctuation-only tail) inserts nothing.
    let final_same = "现在是什么问题？你帮我看一下。";
    assert_eq!(
        pause_early_final_remainder(final_same, delivered, &key(delivered)),
        Some(String::new())
    );
    let final_punct_tail = "现在是什么问题？你帮我看一下！";
    assert_eq!(
        pause_early_final_remainder(final_punct_tail, delivered, &key(delivered)),
        Some(String::new()),
        "a punctuation-only final tail is already covered"
    );

    // Cloud rewrote or shrank the delivered prefix: no safe remainder.
    let rewritten = "现在是什么毛病？你帮我看一下。然后呢。";
    assert_eq!(pause_early_final_remainder(rewritten, delivered, &key(delivered)), None);
    let shrunken = "现在是什么问题？";
    assert_eq!(pause_early_final_remainder(shrunken, delivered, &key(delivered)), None);

    // Empty delivered key means no early delivery: everything remains.
    assert_eq!(
        pause_early_final_remainder(final_text, "", ""),
        Some(final_text.trim().to_string())
    );
}

#[test]
fn final_asr_revision_of_pasted_word_requires_verified_replacement() {
    use super::{pause_early_final_remainder, pause_early_final_revises_delivered_text};
    let delivered = "你不用一直轮询，检查到自动判刑再去查。";
    let final_text = "你不用一直轮询，检查到自动唤醒再去查。";
    let key = super::embedded_audio_partial_preview_stability_key(delivered);

    assert_eq!(pause_early_final_remainder(final_text, delivered, &key), None);
    assert!(pause_early_final_revises_delivered_text(final_text, delivered));
    assert!(pause_early_final_revises_delivered_text("你好！", "你好。"));
    assert!(!pause_early_final_revises_delivered_text("你好，继续。", "你好，"));
}

#[test]
fn pause_early_delivery_waits_for_a_clause_instead_of_pasting_provider_placeholders() {
    use super::{pause_early_chunk_ready, pause_early_complete_clause, pause_early_segment};

    assert!(!pause_early_chunk_ready("", "哎"));
    assert!(!pause_early_chunk_ready("", "Her."));
    assert_eq!(pause_early_segment("然后你要确认", false), None);
    assert_eq!(pause_early_segment("然后你要确认", true), Some("然后你要确认"));
    assert!(pause_early_chunk_ready("", "然后你要确认"));
    assert!(pause_early_chunk_ready("", "然后你要确认，"));
    assert!(!pause_early_chunk_ready("然后你要确认", "的"));
    assert!(pause_early_chunk_ready("然后你要确认", "规划器。"));
    assert_eq!(
        pause_early_segment("第一句，第二句没有标点", true),
        Some("第一句，第二句没有标点")
    );
    assert_eq!(
        pause_early_segment("第一句，第二句没有标点", false),
        Some("第一句，")
    );
    assert_eq!(
        pause_early_complete_clause("然后现在这个。后半句还没说完"),
        Some("然后现在这个。")
    );
    assert_eq!(
        pause_early_complete_clause("第一句，第二句。第三句未完"),
        Some("第一句，第二句。")
    );
    assert_eq!(pause_early_complete_clause("他说：\"可以吗？\"然后"), Some("他说：\"可以吗？\""));
    assert_eq!(pause_early_complete_clause("金额1.06元，后半句"), Some("金额1.06元，"));
    assert_eq!(pause_early_complete_clause("金额1.06"), None);
    assert_eq!(pause_early_complete_clause("尚未形成句子"), None);
}

#[test]
fn pause_early_live_continuation_survives_an_earlier_cloud_word_revision() {
    use super::{
        embedded_audio_partial_preview_stability_key as key,
        pause_early_anchored_continuation, pause_early_final_remainder,
    };

    let first_paste = "今天我们讨论新的语音输入体验";
    let second_snapshot = "今天我们来讨论新的语音输入体验，接下来继续说明实时预览";
    assert_eq!(pause_early_final_remainder(second_snapshot, first_paste, &key(first_paste)), None);
    let second_paste = pause_early_anchored_continuation(second_snapshot, first_paste, &key(first_paste))
        .expect("the unchanged end of the first paste anchors the new words");
    assert_eq!(second_paste, "，接下来继续说明实时预览");

    // Subsequent checks compare against the text actually pasted, not the
    // provider's revised first sentence. This permits a third live segment.
    let displayed = format!("{first_paste}{second_paste}");
    let third_snapshot = "今天我们来讨论新的语音输入体验，接下来继续说明实时预览，然后还有第三段";
    assert_eq!(pause_early_final_remainder(third_snapshot, &displayed, &key(&displayed)), None);
    assert_eq!(
        pause_early_anchored_continuation(third_snapshot, &displayed, &key(&displayed)),
        Some("，然后还有第三段".to_string())
    );

    assert_eq!(
        pause_early_anchored_continuation(
            "今天我们来讨论不同的产品设计，接下来继续说明实时预览",
            first_paste,
            &key(first_paste),
        ),
        None,
        "a rewritten paste boundary must wait for final reconciliation"
    );
}

#[test]
fn pause_early_live_continuation_survives_a_rewritten_opening_with_unique_seam() {
    use super::{embedded_audio_partial_preview_stability_key as key, pause_early_anchored_continuation};

    let delivered = "那句话规划器这个其实拆到这个原料也很快吧然后主要是规划的速度是还是挺快的";
    let revised = "去化规化器这个其实拆到这个原料也很快吧然后主要是规化的速度是还是挺快的然后你就不用对就是反正各中间件";
    assert_eq!(
        pause_early_anchored_continuation(revised, delivered, &key(delivered)),
        Some("然后你就不用对就是反正各中间件".into()),
        "a unique seam beside the paste boundary must keep live text moving"
    );

    let ambiguous_delivered = "完全不同开头甲乙丙丁戊己庚辛";
    let repeated = "另一种开头甲乙丙丁戊己庚辛后面甲乙丙丁戊己庚辛更多";
    assert_eq!(
        pause_early_anchored_continuation(
            repeated,
            ambiguous_delivered,
            &key(ambiguous_delivered)
        ),
        None,
        "a repeated seam cannot safely place the continuation"
    );
}

#[test]
fn pause_early_live_continuation_survives_a_rewritten_paste_seam() {
    use super::{
        embedded_audio_partial_preview_stability_key as key,
        pause_early_aligned_growth_tail, pause_early_anchored_continuation,
        pause_early_final_remainder,
    };

    let delivered = "然后你是验收的话问题都是一直在修对吧？就不是说验收到问题就不修。";
    let revised = "然后你是验收的话问题都是一直在修对吧？就不是说验收到问题就不休。然后这个是怎么样？是治本的修法吗？";
    assert_eq!(pause_early_final_remainder(revised, delivered, &key(delivered)), None);
    assert_eq!(pause_early_anchored_continuation(revised, delivered, &key(delivered)), None);
    assert_eq!(
        pause_early_aligned_growth_tail(revised, &key(delivered)),
        Some("然后这个是怎么样？是治本的修法吗？".to_string()),
        "a stable owner continuation must not wait for the automatic stop when a minor seam revision removes the exact anchor"
    );
}

#[test]
fn pause_early_mismatch_recovery_tail_recovers_clean_rewrites() {
    use super::pause_early_mismatch_recovery_tail;
    let key = |text: &str| super::embedded_audio_partial_preview_stability_key(text);

    // 2026-09-22 21:5x 用户实锤"出来两次"后改版契约:改写只允许在交付
    // 末尾 ≤2 字(标点/同音边界级)时补尾;分界深入交付区间的改写不补——
    // 旧文本已在屏上收不回,补新尾=新旧并存重复(宁少不重复,H 族)。
    let delivered = "今天测试一下停顿录屏";
    let boundary = "今天测试一下停顿落屏。然后补一句。";
    let tail = pause_early_mismatch_recovery_tail(boundary, &key(delivered))
        .expect("boundary rewrite recovers tail");
    assert_eq!(tail, "落屏。然后补一句。");

    // 中段改写(分界后交付区还有 6 字旧内容)但终稿带增长尾 → 2026-09-23
    // 增长尾契约:max(8, 交付/3) 差值内从已交付长度处补尾,屏上旧字不动。
    let deep = "今天测试一下停顿录屏开始点它";
    let deep_final = "今天测试一下停顿落屏，开始点它。然后补一句。";
    assert_eq!(
        pause_early_mismatch_recovery_tail(deep_final, &key(deep)),
        Some("然后补一句。".to_string())
    );

    // 改写太剧烈（LCP 不足一半）→ 放弃，宁少勿乱。
    let heavy = "完全不同的另一句话了。";
    assert_eq!(pause_early_mismatch_recovery_tail(heavy, &key(deep)), None);

    // 终稿比已交付短 → 放弃。
    let shrunken = "今天测试一下。";
    assert_eq!(pause_early_mismatch_recovery_tail(shrunken, &key(deep)), None);

    // 空交付 key → 放弃（正常路径处理）。
    assert_eq!(pause_early_mismatch_recovery_tail(boundary, ""), None);
}

#[test]
fn pause_early_mismatch_recovery_tail_survives_shared_utf8_prefix_divergence() {
    use super::pause_early_mismatch_recovery_tail;
    let key = |text: &str| super::embedded_audio_partial_preview_stability_key(text);

    // 2026-09-23 13:33 panic 实锤（"内部错误"）：报(E6 8A A5)/抱(E6 8A B1)
    // 共享前两字节，字节级 LCP 把切点落在字符内部 → slice panic。修复后
    // 不但不 panic,深改写(区 9>8)叠加微增长(3 字)还走微增长通道补尾。
    let delivered = "为什么没有呢应该有的呀就是你必须要报用内置浏览器看蓝湖";
    let deep_final = "为什么没有呢应该有的呀就是你必须要抱用内置浏览器看蓝湖的链接";
    assert_eq!(
        pause_early_mismatch_recovery_tail(deep_final, &key(delivered)),
        Some("的链接".to_string())
    );

    // 同族字对落在交付末尾(≤2 字,同音边界级)时照常补尾,不 panic。
    let boundary_delivered = "今天测试一下停顿报";
    let boundary_final = "今天测试一下停顿抱。";
    assert_eq!(
        pause_early_mismatch_recovery_tail(boundary_final, &key(boundary_delivered)),
        Some("抱。".to_string())
    );
}

#[test]
fn pause_early_mismatch_recovery_tail_appends_growth_tail_on_late_polish() {
    use super::pause_early_mismatch_recovery_tail;
    let key = |text: &str| super::embedded_audio_partial_preview_stability_key(text);

    // 2026-09-23 14:31/14:32 连续实锤:云端两遍精修润色了中段一个字(分界
    // 距交付末尾 3 字,>2 字边界),旧契约把纯新增的尾巴整段丢弃。改写区在
    // max(8, 交付/3) 以内且终稿更长时,必须从已交付长度处切齐补尾——严格
    // 只追加交付长度之后的内容,不可能重复上屏。
    let delivered = "今天测试一下停顿落屏然后我们继续说说看吧"; // 20 字,分界在"说说"→"说些"(index 17)
    let polished = "今天测试一下停顿落屏然后我们继续说些看吧还有别的";
    assert_eq!(
        pause_early_mismatch_recovery_tail(polished, &key(delivered)),
        Some("还有别的".to_string())
    );

    // 长句(105 字实锤同型):绝对差值口径让 30 字会话里第 22 字的精修
    // (改写区 8 = max(8, 30/3))也能补上增长尾。
    let long_delivered = "今天测试一下停顿落屏然后我们继续说说看吧还有别的办法可以试试看呢"; // 30 字
    let long_polished = "今天测试一下停顿落屏然后我们继续说说看吧还有特的办法可以试试看呢对吧";
    assert_eq!(
        pause_early_mismatch_recovery_tail(long_polished, &key(long_delivered)),
        Some("对吧".to_string())
    );

    // 微增长(≤3 字)无条件补:2026-09-23 15:29 实锤 23→24 深改写吞 1 字。
    // 改写区可以任意深,1-3 字的尾巴不可能是重述。
    let micro_delivered = "今天测试一下停顿落屏然后我们继续说说看吧还有别的"; // 24 字
    let micro_polished = "今天测试一下停顿落屏另外我们后来继续说说看吧还有别的呀"; // 分界在第 10 字(改写区 14>8),增长 3 字
    assert_eq!(
        pause_early_mismatch_recovery_tail(micro_polished, &key(micro_delivered)),
        Some("呀".to_string())
    );

    // 深改写重述("出来两次"型:改写区超限且增长 >3 字)即使终稿更长也不补
    // ——新旧并存重复比丢尾更伤,维持 None。增长 ≤3 字的走微增长通道。
    let restated = "今天测试一下现在是完全不同的另一句话内容更长了呀真的";
    assert_eq!(
        pause_early_mismatch_recovery_tail(restated, &key(delivered)),
        None
    );
}

#[test]
fn pause_early_rollback_restores_confirmed_prefix_and_keeps_sticky_floor() {
    use super::{
        pause_early_delivery_confirm, pause_early_delivery_reserve,
        pause_early_delivery_rollback, pause_early_ever_delivered, take_pause_early_delivery,
    };

    let coordinator = Coordinator::new();
    let inner = &coordinator.inner;
    let session_id = uuid::Uuid::new_v4();

    // 首贴成功:已确认前缀入账,粘滞底线上闩。
    pause_early_delivery_reserve(
        inner,
        session_id,
        "你好世界".to_string(),
        "你好世界".to_string(),
    );
    pause_early_delivery_confirm(inner, session_id);
    assert!(pause_early_ever_delivered(inner, session_id));

    // 第二贴失败:回滚只退这一次未确认的交付,已上屏的首段必须保留在账本
    // ——整段抹掉会让终稿全文重贴(2026-09-23 用户实锤"一毛一样粘贴两次")。
    pause_early_delivery_reserve(
        inner,
        session_id,
        "你好世界然后继续".to_string(),
        "你好世界然后继续".to_string(),
    );
    pause_early_delivery_rollback(inner, session_id);
    let (display, key) =
        take_pause_early_delivery(inner, session_id).expect("confirmed prefix survives rollback");
    assert_eq!(display, "你好世界");
    assert_eq!(key, "你好世界");

    // 粘滞底线是历史事实:不因 take 消费账本而翻转(终稿先读底线再 take,
    // take 后仍可查询)。
    assert!(pause_early_ever_delivered(inner, session_id));
}

#[test]
fn pause_early_rollback_on_first_paste_returns_to_empty_ledger() {
    use super::{
        pause_early_delivery_rollback, pause_early_delivery_reserve,
        pause_early_ever_delivered, take_pause_early_delivery,
    };

    let coordinator = Coordinator::new();
    let inner = &coordinator.inner;
    let session_id = uuid::Uuid::new_v4();

    // 首贴就失败:没有任何已上屏文本,账本必须归零,终稿走全文交付。
    pause_early_delivery_reserve(
        inner,
        session_id,
        "你好世界".to_string(),
        "你好世界".to_string(),
    );
    pause_early_delivery_rollback(inner, session_id);
    assert!(take_pause_early_delivery(inner, session_id).is_none());
    assert!(!pause_early_ever_delivered(inner, session_id));
}

#[test]
fn pause_early_tail_beyond_delivered_anchor_recovers_tail_when_lengths_distort() {
    use super::pause_early_tail_beyond_delivered_anchor;
    let key = |text: &str| super::embedded_audio_partial_preview_stability_key(text);

    // 2026-09-23 17:31 d37e3562 吞尾实锤：流式中间态膨胀（的了/呢/点点）
    // 让前缀早期分叉、长度域不可比，但锚区"慢了你看一下是为"两域一致，
    // 锚后的"什么？然后我现在继续说话。"从未上屏——必须补。
    let delivered = "现在是你在盯着监控看的了是吧然后呢感觉这个唤醒有一点点慢了你看一下是为";
    let final_text =
        "现在是你在盯着监控看的是吧？然后感觉这个唤醒是有点慢了，你看一下是为什么？然后我现在继续说话。";
    assert_eq!(
        pause_early_tail_beyond_delivered_anchor(final_text, &key(delivered)),
        Some("什么？然后我现在继续说话。".to_string()),
    );

    // 锚点落在终稿末尾（锚后无新内容）＝已交付覆盖到头，无尾可补。
    let covered = "现在是什么问题你帮我看一下";
    let final_covered = "现在是什么问题？你帮我看一下。";
    assert_eq!(
        pause_early_tail_beyond_delivered_anchor(final_covered, &key(covered)),
        None,
    );

    // 锚串在终稿中不存在（结尾区域被两遍改写）→ 维持跳过。
    let diverged_tail = "开头一致但是结尾甲乙丙丁戊己";
    let final_diverged = "开头一致但是结尾被彻底重写了另一份内容。";
    assert_eq!(
        pause_early_tail_beyond_delivered_anchor(final_diverged, &key(diverged_tail)),
        None,
    );
}

#[test]
fn pause_early_unique_long_seam_survives_a_rewritten_opening() {
    use super::pause_early_tail_beyond_delivered_anchor;
    let key = |text: &str| super::embedded_audio_partial_preview_stability_key(text);

    // Fresh 1.0.6 session 67577453: two-pass ASR changed the opening from
    // the first character but retained the clause ending next to the paste
    // boundary. The earlier opening-only guard dropped the entire next clause.
    let delivered = "那句话规划器这个其实拆到这个原料也很快吧然后主要是规划的速度是还是挺快的";
    let final_text = "去化规化器这个其实拆到这个原料也很快吧然后主要是规化的速度是还是挺快的然后你就不用对就是反正各中间件";
    assert_eq!(
        pause_early_tail_beyond_delivered_anchor(final_text, &key(delivered)),
        Some("然后你就不用对就是反正各中间件".into()),
    );

    // A repeated seam cannot prove which occurrence was already on screen.
    let ambiguous = "完全不同开头甲乙丙丁甲乙丙丁";
    let repeated = "另一种开头甲乙丙丁甲乙丙丁后面甲乙丙丁甲乙丙丁更多";
    assert_eq!(
        pause_early_tail_beyond_delivered_anchor(repeated, &key(ambiguous)),
        None,
    );
}

#[test]
fn pause_early_alignment_recovers_continuation_after_middle_and_boundary_revisions() {
    use super::{
        pause_early_aligned_growth_tail, pause_early_final_remainder,
        pause_early_mismatch_recovery_tail, pause_early_tail_beyond_delivered_anchor,
    };
    let key = |text: &str| super::embedded_audio_partial_preview_stability_key(text);
    // A live preview misheard one word in the middle and its last word. The
    // final has a genuine new sentence, but neither an exact prefix nor an
    // exact terminal anchor survives. Re-pasting the whole final duplicates
    // the early delivery; dropping it loses the continuation.
    let delivered = "现在请检查设备状况我想确认它已经连接然后呢";
    let final_text = "现在请检查设备状态我想确认它已经连接然后我继续说明下一步的具体安排";
    let delivered_key = key(delivered);
    assert_eq!(pause_early_final_remainder(final_text, delivered, &delivered_key), None);
    assert_eq!(pause_early_mismatch_recovery_tail(final_text, &delivered_key), None);
    assert_eq!(pause_early_tail_beyond_delivered_anchor(final_text, &delivered_key), None);
    assert_eq!(
        pause_early_aligned_growth_tail(final_text, &delivered_key),
        Some("继续说明下一步的具体安排".to_string())
    );
}

#[test]
fn pause_early_alignment_rejects_ambiguous_rewrites_and_internal_growth() {
    use super::pause_early_aligned_growth_tail;
    let key = |text: &str| super::embedded_audio_partial_preview_stability_key(text);
    let delivered = "今天测试一下停顿落屏然后我们继续说说看吧";
    let unrelated = "今天测试现在有不同的事情要讲而且后面还有很多新的内容需要补充";
    assert_eq!(pause_early_aligned_growth_tail(unrelated, &key(delivered)), None);

    // Extra words inserted inside the already delivered text are a cloud
    // revision, not evidence of a new tail.
    let delivered = "现在我们看一下这个流程然后继续下一步";
    let internal_growth = "现在我们看一下这个复杂而详细的流程然后继续下一步";
    assert_eq!(
        pause_early_aligned_growth_tail(internal_growth, &key(delivered)),
        None
    );
    let long_delivered = "现在我们看一下这个流程然后继续下一步接着验证整个设备状态是否正常";
    let long_internal_growth =
        "现在我们看一下这个复杂流程然后继续下一步接着验证整个设备状态是否正常";
    assert_eq!(
        pause_early_aligned_growth_tail(long_internal_growth, &key(long_delivered)),
        None
    );
}
