// EmbeddedStreamingDictation actor / PCM submit path.
// Included into `coordinator::dictation` via `include!`.

use std::io::Write;

const BLE_CANDIDATE_INGRESS_DIAGNOSTIC_CHANNEL_CAPACITY: usize = 512;

fn wake_anchor_mode(
    wake_start_seconds: Option<f32>,
    wake_end_seconds: f32,
) -> &'static str {
    match wake_start_seconds {
        None => "fallback_missing_start",
        Some(start) if start.is_finite() && start >= 0.0 && start <= wake_end_seconds => {
            "keyword_start"
        }
        Some(_) => "fallback_invalid_start",
    }
}

fn log_wake_candidate_crop(
    path: &'static str,
    embedded_session_id: u32,
    candidate: &BufferedSpeakerCandidate,
    wake_match: &crate::wake_phrase::Match,
    candidate_pcm_before: usize,
    pcm_base_before: Option<u64>,
    post_wake_offset: usize,
    wake_anchor_offset: usize,
) {
    let candidate_pcm_after = candidate.pcm.len();
    let expected_after = candidate_pcm_before.saturating_sub(wake_anchor_offset);
    let first_capture_generation = candidate
        .source_runs
        .iter()
        .find_map(|run| run.capture_generation);
    let last_capture_generation = candidate
        .source_runs
        .iter()
        .rev()
        .find_map(|run| run.capture_generation);
    log::info!(
        "[wake-phrase] candidate crop path={} embedded_session_id={} candidate_id={} kind={:?} capture_generation_first={:?} capture_generation_last={:?} wake_start_s={:?} wake_end_s={:.3} anchor_mode={} candidate_bytes_before={} wake_anchor_offset_bytes={} post_wake_offset_bytes={} candidate_bytes_after={} expected_after_bytes={} crop_invariant={} pcm_base_before={:?} pcm_base_after={:?} candidate_range_before={:?} candidate_range_after={:?}",
        path,
        embedded_session_id,
        candidate.candidate_id,
        candidate.kind,
        first_capture_generation,
        last_capture_generation,
        wake_match.start_seconds,
        wake_match.end_seconds,
        wake_anchor_mode(wake_match.start_seconds, wake_match.end_seconds),
        candidate_pcm_before,
        wake_anchor_offset,
        post_wake_offset,
        candidate_pcm_after,
        expected_after,
        candidate_pcm_after == expected_after,
        pcm_base_before,
        candidate.pcm_base_offset,
        candidate_slice_range(pcm_base_before, 0, candidate_pcm_before),
        candidate_slice_range(candidate.pcm_base_offset, 0, candidate_pcm_after),
    );
}

enum BleCandidateIngressDiagnosticMessage {
    Append {
        capture_generation: u64,
        candidate_id: u64,
        candidate_kind: String,
        session_id: u32,
        packet_sequence: u16,
        candidate_range: Option<crate::observability::CandidateRange>,
        collector_metadata: Option<crate::embedded_audio::StreamingPcmChunkMetadata>,
        capture_fact: Option<crate::embedded_audio::SessionAdmissionFact>,
        actor_fact: Option<crate::embedded_audio::SessionAdmissionFact>,
        appended_unix_ms: u64,
        pcm: Vec<u8>,
    },
}

#[derive(Clone)]
struct BleCandidateIngressDiagnosticSink {
    tx: std::sync::mpsc::SyncSender<BleCandidateIngressDiagnosticMessage>,
    dropped_count: Arc<AtomicUsize>,
}

impl BleCandidateIngressDiagnosticSink {
    fn for_capture_generation(capture_generation: u64) -> Option<Self> {
        let directory = explicit_wake_diagnostic_directory(
            std::env::var(WAKE_DIAGNOSTIC_DIR_ENV).ok(),
        )?;
        let timestamp_ms = ble_candidate_ingress_diagnostic_unix_ms();
        let stem = format!(
            "ble-ingress-candidate-capture-{capture_generation}-{}-{timestamp_ms}",
            std::process::id()
        );
        let events_path = directory.join(format!("{stem}-events.jsonl"));
        let pcm_path = directory.join(format!("{stem}.pcm"));
        let (tx, rx) = std::sync::mpsc::sync_channel(
            BLE_CANDIDATE_INGRESS_DIAGNOSTIC_CHANNEL_CAPACITY,
        );
        let dropped_count = Arc::new(AtomicUsize::new(0));
        let dropped_count_for_worker = Arc::clone(&dropped_count);
        let spawn = std::thread::Builder::new()
            .name(format!(
                "listener-type-ble-candidate-ingress-{capture_generation}"
            ))
            .spawn(move || {
                run_ble_candidate_ingress_diagnostic_worker(
                    capture_generation,
                    directory,
                    events_path,
                    pcm_path,
                    rx,
                    dropped_count_for_worker,
                );
            });
        if let Err(err) = spawn {
            log::warn!(
                "[speaker-verification] candidate ingress diagnostic worker unavailable capture_generation={capture_generation}: {err}"
            );
            return None;
        }
        Some(Self { tx, dropped_count })
    }

    fn record_append(
        &self,
        capture_generation: u64,
        candidate_id: u64,
        candidate_kind: BufferedSpeakerCandidateKind,
        session_id: u32,
        packet_sequence: u16,
        candidate_range: Option<crate::observability::CandidateRange>,
        collector_metadata: Option<crate::embedded_audio::StreamingPcmChunkMetadata>,
        capture_fact: Option<crate::embedded_audio::SessionAdmissionFact>,
        actor_fact: Option<crate::embedded_audio::SessionAdmissionFact>,
        pcm: &[u8],
    ) {
        let message = BleCandidateIngressDiagnosticMessage::Append {
            capture_generation,
            candidate_id,
            candidate_kind: format!("{candidate_kind:?}"),
            session_id,
            packet_sequence,
            candidate_range,
            collector_metadata,
            capture_fact,
            actor_fact,
            appended_unix_ms: ble_candidate_ingress_diagnostic_unix_ms(),
            pcm: pcm.to_vec(),
        };
        if self.tx.try_send(message).is_err()
            && self.dropped_count.fetch_add(1, Ordering::Relaxed) == 0
        {
            log::warn!(
                "[speaker-verification] candidate ingress diagnostic queue dropped at least one record; audio flow is unaffected"
            );
        }
    }
}

fn ble_candidate_ingress_diagnostic_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

fn ble_candidate_ingress_diagnostic_sinks(
) -> &'static std::sync::Mutex<
    std::collections::BTreeMap<u64, BleCandidateIngressDiagnosticSink>,
> {
    static SINKS: OnceLock<
        std::sync::Mutex<
            std::collections::BTreeMap<u64, BleCandidateIngressDiagnosticSink>,
        >,
    > = OnceLock::new();
    SINKS.get_or_init(|| std::sync::Mutex::new(std::collections::BTreeMap::new()))
}

fn ble_candidate_ingress_diagnostic_sink(
    capture_generation: u64,
) -> Option<BleCandidateIngressDiagnosticSink> {
    let sinks = ble_candidate_ingress_diagnostic_sinks();
    let mut sinks = sinks.lock().ok()?;
    if let Some(sink) = sinks.get(&capture_generation) {
        return Some(sink.clone());
    }
    while sinks.len() >= 8 {
        let oldest = sinks.keys().next().copied()?;
        sinks.remove(&oldest);
    }
    let sink = BleCandidateIngressDiagnosticSink::for_capture_generation(capture_generation)?;
    sinks.insert(capture_generation, sink.clone());
    Some(sink)
}

fn ble_candidate_ingress_admission_fact_json(
    fact: Option<&crate::embedded_audio::SessionAdmissionFact>,
) -> serde_json::Value {
    let Some(fact) = fact else {
        return serde_json::Value::Null;
    };
    serde_json::json!({
        "collectorInstanceId": fact.collector_instance_id,
        "resetEpoch": fact.reset_epoch,
        "notificationId": fact.notification_id,
        "admissionId": fact.admission_id,
        "physicalSessionId": fact.physical_session_id,
        "packetSequence": fact.packet_sequence,
        "disposition": serde_json::to_value(fact.disposition).unwrap_or(serde_json::Value::Null),
        "supersedesAdmissionId": fact.supersedes_admission_id,
        "predecessorUnknown": fact.predecessor_unknown,
        "afterStopBoundary": fact.after_stop_boundary,
        "wirePayloadBytes": fact.wire_payload_bytes,
        "declaredPcmBytes": fact.declared_pcm_bytes,
        "expandedPcmBytes": fact.expanded_pcm_bytes,
        "metadataIncomplete": fact.metadata_incomplete,
    })
}

fn write_ble_candidate_ingress_json_line(
    writer: &mut std::io::BufWriter<std::fs::File>,
    value: serde_json::Value,
) -> bool {
    if serde_json::to_writer(&mut *writer, &value).is_err()
        || writer.write_all(b"\n").is_err()
        || writer.flush().is_err()
    {
        return false;
    }
    true
}

fn run_ble_candidate_ingress_diagnostic_worker(
    capture_generation: u64,
    directory: std::path::PathBuf,
    events_path: std::path::PathBuf,
    pcm_path: std::path::PathBuf,
    rx: std::sync::mpsc::Receiver<BleCandidateIngressDiagnosticMessage>,
    dropped_count: Arc<AtomicUsize>,
) {
    if let Err(err) = fs::create_dir_all(&directory) {
        log::warn!(
            "[speaker-verification] candidate ingress diagnostic directory unavailable capture_generation={capture_generation}: {err}"
        );
        return;
    }
    let Ok(events_file) = fs::File::create(&events_path) else {
        log::warn!(
            "[speaker-verification] candidate ingress events file unavailable capture_generation={} path={}",
            capture_generation,
            events_path.display()
        );
        return;
    };
    let Ok(mut pcm_file) = fs::File::create(&pcm_path) else {
        log::warn!(
            "[speaker-verification] candidate ingress PCM file unavailable capture_generation={} path={}",
            capture_generation,
            pcm_path.display()
        );
        return;
    };
    let mut events = std::io::BufWriter::new(events_file);
    let mut saved_pcm_bytes = 0usize;
    while let Ok(message) = rx.recv() {
        let BleCandidateIngressDiagnosticMessage::Append {
            capture_generation,
            candidate_id,
            candidate_kind,
            session_id,
            packet_sequence,
            candidate_range,
            collector_metadata,
            capture_fact,
            actor_fact,
            appended_unix_ms,
            pcm,
        } = message;
        let pcm_hash = crate::firmware_ota::sha256_hex(&pcm);
        let remaining = WAKE_DIAGNOSTIC_MAX_PCM_BYTES.saturating_sub(saved_pcm_bytes);
        let save_bytes = remaining.min(pcm.len());
        if save_bytes > 0 {
            if let Err(err) = pcm_file.write_all(&pcm[..save_bytes]) {
                log::warn!(
                    "[speaker-verification] candidate ingress PCM append failed capture_generation={} path={}: {err}",
                    capture_generation,
                    pcm_path.display()
                );
                saved_pcm_bytes = WAKE_DIAGNOSTIC_MAX_PCM_BYTES;
            } else {
                let _ = pcm_file.flush();
                saved_pcm_bytes = saved_pcm_bytes.saturating_add(save_bytes);
            }
        }
        let _ = write_ble_candidate_ingress_json_line(
            &mut events,
            serde_json::json!({
                "boundary": "candidate_append",
                "captureGeneration": capture_generation,
                "candidateId": candidate_id,
                "candidateKind": candidate_kind,
                "sessionId": session_id,
                "packetSequence": packet_sequence,
                "appendedUnixMs": appended_unix_ms,
                "candidateRange": candidate_range,
                "collectorMetadata": collector_metadata,
                "captureAdmissionFact": ble_candidate_ingress_admission_fact_json(capture_fact.as_ref()),
                "actorAdmissionFact": ble_candidate_ingress_admission_fact_json(actor_fact.as_ref()),
                "pcmBytes": pcm.len(),
                "pcmSha256": pcm_hash,
                "pcmSavedBytes": save_bytes,
                "pcmSaveTruncated": save_bytes < pcm.len(),
                "pcmPath": pcm_path,
                "diagnosticQueueDroppedCount": dropped_count.load(Ordering::Relaxed),
            }),
        );
    }
}

fn record_ble_candidate_ingress_append(
    source_context: Option<&CaptureAdmissionSourceContext>,
    candidate_id: u64,
    candidate_kind: BufferedSpeakerCandidateKind,
    session_id: u32,
    packet_sequence: u16,
    candidate_range: Option<crate::observability::CandidateRange>,
    collector_metadata: Option<crate::embedded_audio::StreamingPcmChunkMetadata>,
    pcm: &[u8],
) {
    let Some(source_context) = source_context else {
        return;
    };
    let Some(capture_generation) = source_context.capture_generation else {
        return;
    };
    let Some(sink) = ble_candidate_ingress_diagnostic_sink(capture_generation) else {
        return;
    };
    sink.record_append(
        capture_generation,
        candidate_id,
        candidate_kind,
        session_id,
        packet_sequence,
        candidate_range,
        collector_metadata,
        source_context.capture_fact.clone(),
        source_context.actor_fact.clone(),
        pcm,
    );
}

impl EmbeddedStreamingDictation {
    async fn apply_ble_packet_actor_command(
        &mut self,
        inner: &Arc<Inner>,
        event: crate::embedded_audio::StreamingSessionEvent,
        source_observation: Option<
            Arc<crate::observability::EmbeddedAudioPipelineObservation>,
        >,
        source_observation_is_explicit: bool,
        source_admission_context: Option<CaptureAdmissionSourceContext>,
    ) -> Result<bool, String> {
        self.last_actor_pcm_consumed = false;
        match event {
            crate::embedded_audio::StreamingSessionEvent::Started { session_id, origin } => {
                // A failed orphan-tail recovery is quarantined by session id.
                // Do not let a replayed SessionStart immediately reopen the
                // same poisoned segment; only a new firmware session id may
                // clear the tombstone.
                if self
                    .orphan_recovery_quarantine_session_id
                    .is_some_and(|quarantined| quarantined == session_id)
                {
                    log::warn!(
                        "[coord] coalescing quarantined embedded SessionStart embedded_session_id={session_id} origin={origin:?}"
                    );
                    return Ok(false);
                }
                if self
                    .orphan_recovery_quarantine_session_id
                    .is_some_and(|quarantined| quarantined != session_id)
                {
                    log::info!(
                        "[coord] advancing orphan recovery quarantine old_session_id={:?} new_session_id={session_id}",
                        self.orphan_recovery_quarantine_session_id
                    );
                    self.orphan_recovery_quarantine_session_id = None;
                }
                if !recording_gate::try_admit_arc(
                    inner,
                    RecordIntent::BleSessionStart,
                    &format!(
                        "embedded start embedded_session_id={session_id} origin={origin:?}"
                    ),
                ) {
                    return Ok(false);
                }
                self.begin_candidate_or_session(inner, session_id, origin)
                    .await?;
                Ok(false)
            }
            crate::embedded_audio::StreamingSessionEvent::PcmChunk(chunk) => {
                if !recording_gate::try_admit_arc(
                    inner,
                    RecordIntent::BlePcmIngest,
                    &format!("embedded pcm embedded_session_id={}", chunk.session_id),
                ) {
                    // Drop any late packets from a session that started before OTA pause.
                    if self.session.is_some() || self.speaker_candidate.is_some() {
                        log::info!(
                            "[recording-gate] dropping in-flight embedded session on deny embedded_session_id={}",
                            chunk.session_id
                        );
                        self.close_owned_product_lifecycle(inner);
                        if let Some(mut session) = self.session.take() {
                            self.preserve_session_candidate_fact_ledger(&mut session);
                        }
                        if let Some(mut candidate) = self.speaker_candidate.take() {
                            let discarded_bytes = candidate.pcm.len();
                            candidate.record_outcome_fact(
                                self.pipeline_observation.as_ref(),
                                crate::observability::CandidateFactKind::Discarded,
                                "recording_gate_denied",
                                discarded_bytes,
                            );
                            candidate.record_outcome_fact(
                                self.pipeline_observation.as_ref(),
                                crate::observability::CandidateFactKind::Closed,
                                "recording_gate_denied",
                                0,
                            );
                            self.preserve_candidate_fact_ledger(&mut candidate);
                        }
                        self.embedded_session_id = None;
                        self.pending_stop_expected_packet_count = None;
                    }
                    return Ok(false);
                }
                let chunk_session_id = chunk.session_id;
                // The session observation is only the fact sink. A candidate's
                // physical source must come from this packet's explicit
                // observation; falling back to the previous session observation
                // would rebind an unobserved packet to an old generation.
                let candidate_observation = self
                    .pipeline_observation
                    .clone()
                    .or_else(|| source_observation.clone());
                let candidate_source_observation = source_observation_is_explicit
                    .then_some(source_observation.clone())
                    .flatten();
                if let Some(candidate) = self.speaker_candidate.as_mut() {
                    if candidate.kind == BufferedSpeakerCandidateKind::Rejected {
                        return Ok(false);
                    }
                    if candidate.pcm.len().saturating_add(chunk.pcm.len())
                        > MAX_BUFFERED_SPEAKER_CANDIDATE_BYTES
                    {
                        candidate.record_fact(
                            candidate_observation.as_ref(),
                            crate::observability::CandidateFactKind::Rejected,
                            candidate.current_candidate_range(),
                            None,
                            None,
                            None,
                            None,
                            Vec::new(),
                            chunk.pcm.len(),
                            Some("candidate_buffer_limit"),
                        );
                        return Err("声纹候选录音超过安全缓冲上限".to_string());
                    }
                    let candidate_range = candidate
                        .candidate_cursor
                        .append(chunk.pcm.len())
                        .map(|range| crate::observability::CandidateRange {
                            start: range.start,
                            end: range.end,
                        });
                    record_ble_candidate_ingress_append(
                        source_admission_context.as_ref(),
                        candidate.candidate_id,
                        candidate.kind,
                        chunk_session_id,
                        chunk.packet_sequence,
                        candidate_range,
                        chunk.metadata,
                        &chunk.pcm,
                    );
                    candidate.pcm.extend_from_slice(&chunk.pcm);
                    self.last_actor_pcm_consumed = true;
                    candidate.source_runs.push_back(BufferedCandidateSourceRun {
                        bytes: chunk.pcm.len(),
                        capture_generation: candidate_source_observation
                            .as_ref()
                            .map(|observation| observation.capture_generation()),
                        segment_id: Some(chunk_session_id),
                        candidate_range,
                        collector_metadata: chunk.metadata,
                        collector_emitted_range: chunk
                            .metadata
                            .map(|metadata| metadata.emitted_range),
                        capture_admission_context: source_admission_context.clone(),
                    });
                    let source_fact = crate::observability::CandidateSourceRunFact {
                        capture_generation: candidate_source_observation
                            .as_ref()
                            .map(|observation| observation.capture_generation()),
                        segment_id: Some(chunk_session_id),
                        candidate_range,
                        collector_metadata: chunk.metadata,
                        collector_emitted_range: chunk
                            .metadata
                            .map(|metadata| metadata.emitted_range),
                        bytes: chunk.pcm.len() as u64,
                    };
                    candidate.record_fact(
                        candidate_observation.as_ref(),
                        crate::observability::CandidateFactKind::Buffered,
                        candidate_range,
                        None,
                        None,
                        None,
                        None,
                        vec![source_fact],
                        chunk.pcm.len(),
                        None,
                    );
                    let source_admission_operation_id =
                        candidate.next_source_admission_operation_id();
                    record_source_admission_dependency(
                        &candidate.source_admission_ledger,
                        source_admission_context.as_ref(),
                        SourceAdmissionOperationOwner::Candidate {
                            candidate_id: candidate.candidate_id,
                        },
                        SourceAdmissionUse::CandidateGateInput,
                        candidate_range.map(|range| SourceAdmissionOwnerRange::Candidate {
                            start: range.start,
                            end: range.end,
                        }),
                        None,
                        chunk.pcm.len(),
                        source_admission_operation_id,
                    );
                    if candidate.kind == BufferedSpeakerCandidateKind::Enrollment { crate::speaker_verification::observe_enrollment_capture(&candidate.pcm); }
                    #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
                    if candidate.kind == BufferedSpeakerCandidateKind::Verification {
                        let phrase = inner.prefs.get().voice_wake_phrase;
                        maybe_start_target_wake_extraction(
                            candidate,
                            &phrase,
                            chunk_session_id,
                        );
                    }
                    if self
                        .promote_hidden_candidate_if_requested(inner, chunk_session_id)
                        .await?
                    {
                        return Ok(false);
                    }
                    if self
                        .try_release_automatic_candidate(inner, chunk_session_id)
                        .await?
                    {
                        return Ok(false);
                    }
                    if let Some(expected_packet_count) = self.pending_stop_expected_packet_count {
                        if self.collector.inner().has_successful_complete_session() {
                            self.finish_completed_streaming_session(
                                inner,
                                chunk_session_id,
                                expected_packet_count,
                            )
                            .await?;
                            return Ok(true);
                        }
                    }
                    return Ok(false);
                }
                // Explicit SessionStart (User / VoiceActivation) must create
                // `session` or `speaker_candidate` first. Mid-stream PCM after Type
                // restart / notify reopen used to call begin_session_if_needed here
                // and open a full Recording capsule with no wake/key intent (owner
                // saw phantom dictation; stop origin was still VoiceActivation).
                if self.session.is_none() {
                    // A notify/GATT reopen can deliver the middle of a
                    // VoiceActivation segment before its SessionStart frame
                    // (the firmware may already be draining a pre-roll burst).
                    // Dropping every orphan PCM packet makes that entire wake
                    // attempt invisible and produces the intermittent
                    // "sometimes wakes, sometimes does nothing" failure.  A
                    // recovered segment is still a hidden Verification
                    // candidate, never a visible dictation session; the normal
                    // phrase + owner gates therefore remain authoritative.
                    if self.orphan_recovery_quarantine_session_id == Some(chunk.session_id) {
                        if chunk.packet_sequence < 4 || chunk.packet_sequence % 500 == 0 {
                            log::warn!(
                                "[coord] dropping quarantined orphan embedded PCM embedded_session_id={} packet_sequence={} (waiting for a new firmware session id)",
                                chunk.session_id,
                                chunk.packet_sequence
                            );
                        }
                        return Ok(false);
                    }
                    if chunk.packet_sequence >= 3
                        && self.embedded_session_id.is_none()
                        && self.speaker_candidate.is_none()
                    {
                        log::warn!(
                            "[coord] recovering orphan embedded PCM as hidden VoiceActivation candidate embedded_session_id={} packet_sequence={} pcm_bytes={} (SessionStart was lost during notify reopen)",
                            chunk.session_id,
                            chunk.packet_sequence,
                            chunk.pcm.len()
                        );
                        self.orphan_recovery_quarantine_session_id = Some(chunk.session_id);
                        match self.begin_candidate_or_session(
                            inner,
                            chunk.session_id,
                            crate::embedded_audio::SessionStartOrigin::VoiceActivation,
                        )
                        .await
                        {
                            Ok(()) => {
                                // Recovery was admitted. The quarantine is no
                                // longer needed; the candidate now owns the
                                // session and normal lifecycle errors apply.
                                self.orphan_recovery_quarantine_session_id = None;
                            }
                            Err(err) => {
                                // This is a transport-boundary failure, not a
                                // speech decision. Keep notify alive and drop
                                // the poisoned segment instead of returning an
                                // error for every replayed packet.
                                log::warn!(
                                    "[coord] orphan embedded PCM recovery rejected once; quarantining session embedded_session_id={} error={err}",
                                    chunk.session_id
                                );
                                self.reset_for_next_session();
                            }
                        }
                        // The current packet is intentionally not replayed into
                        // the detector: the detector is initialized
                        // asynchronously and the next packet preserves the
                        // actor's single-flight ordering.  It is at most one
                        // 10–15 ms frame, not an entire lost pre-roll.
                        return Ok(false);
                    }
                    // High-rate orphan tails after notify reopen can flood the log
                    // and stall the stream actor; keep first packets only.
                    if chunk.packet_sequence < 3 || chunk.packet_sequence % 500 == 0 {
                        log::info!(
                            "[coord] ignoring orphan embedded PCM without explicit start embedded_session_id={} packet_sequence={} pcm_bytes={} (no phantom recording on reconnect)",
                            chunk.session_id,
                            chunk.packet_sequence,
                            chunk.pcm.len()
                        );
                    }
                    return Ok(false);
                }
                // A continuation is admitted only after the request-tagged
                // SessionStart marker has been observed.  A raw PCM packet is
                // deliberately not enough evidence: notify reordering can
                // otherwise bind an unrelated manual segment to this wake.
                if let Some(ensure) = self.accepted_wake_capture_ensure {
                    if ensure.confirmed_segment_id == Some(chunk_session_id)
                        && self.embedded_session_id != Some(chunk_session_id)
                    {
                        self.bind_post_activation_segment(inner, chunk_session_id, false);
                        log::info!(
                            "[wake-phrase] ENSURE-confirmed continuation PCM bound embedded_session_id={chunk_session_id} request_id={}",
                            ensure.request_id
                        );
                    }
                }
                if self.accepted_wake_capture_ensure.is_none() {
                    if let Some((pre_segment_id, _)) = self.activation_segment_race_guard {
                        if chunk_session_id != pre_segment_id {
                            self.bind_post_activation_segment(inner, chunk_session_id, true);
                            log::info!(
                                "[coord] dictation session lazily bound to post-activation embedded segment embedded_session_id={chunk_session_id}"
                            );
                        }
                    }
                }
                    if embedded_streaming_chunk_is_asr_input(&chunk) {
                        self.begin_session_if_needed(inner, chunk.session_id)
                            .await?;
                        let proactive_stop_due = {
                            let session = self
                                .session
                                .as_mut()
                                .ok_or_else(|| "嵌入式音频流式听写 session 尚未创建".to_string())?;
                            let accepted_destination_start = session.accepted_pcm_cursor.position();
                            let streamed_pcm_bytes_before = session.streamed_pcm_bytes;
                            crate::observability::record_embedded_audio_first_packet(
                                session.session_id,
                            );
                            let accepted_source_interval = session
                                .consume_streaming_pcm_from_segment_with_collector_metadata(
                                inner,
                                &chunk.pcm,
                                chunk.raw_input_level_percent,
                                Some(chunk_session_id),
                                source_observation.clone(),
                                source_observation_is_explicit,
                                chunk.metadata,
                            )?;
                            // This method returns Ok(()) for inactive or
                            // stop-rejected sessions. The accepted-byte
                            // boundary is the product fact; diagnostic
                            // cursors are not an acceptance signal.
                            let accepted_bytes = session
                                .streamed_pcm_bytes
                                .saturating_sub(streamed_pcm_bytes_before);
                            self.last_actor_pcm_consumed = accepted_bytes > 0;
                            if accepted_bytes > 0 {
                                let source_admission_operation_id =
                                    session.next_source_admission_operation_id();
                                let accepted_destination_range = accepted_destination_start
                                    .zip(session.accepted_pcm_cursor.position())
                                    .and_then(|(start, end)| {
                                        (end >= start
                                            && end - start == accepted_bytes as u64)
                                            .then_some(crate::observability::PcmRange {
                                                start,
                                                end,
                                            })
                                    });
                                record_source_admission_dependency(
                                    &session.source_admission_ledger,
                                    source_admission_context.as_ref(),
                                    SourceAdmissionOperationOwner::Session {
                                        session_id: session.session_id,
                                    },
                                    SourceAdmissionUse::SessionBodyInput,
                                    accepted_destination_range.map(|range| {
                                        SourceAdmissionOwnerRange::Session {
                                            start: range.start,
                                            end: range.end,
                                        }
                                    }),
                                    accepted_source_interval,
                                    accepted_bytes,
                                    source_admission_operation_id,
                                );
                            }
                            // Normally endpointing is content-aware and driven by
                            // ASR/speaker callbacks. If the provider failed, use
                            // only the local safety fallback to avoid an endless
                            // recording with no output path.
                            let asr_delivery_failed = session
                                .volcengine_asr
                                .as_ref()
                                .is_some_and(|asr| asr.audio_delivery_failed());
                            let silence_threshold_ms =
                                proactive_stop_silence_threshold_ms(asr_delivery_failed);
                            session.proactive_stop_body_started
                                && session.proactive_stop_silence_ms >= silence_threshold_ms
                                && !session.proactive_stop_dispatched
                        };
                    if proactive_stop_due {
                        // Mirror stop_dictation: ask the firmware to cut the session
                        // short. The device then emits Stopped, which drives the normal
                        // finish_completed_streaming_session path — no bespoke finish.
                        let stop_sent = request_embedded_ble_recording_stop_from_host(
                            inner,
                            "provider_delivery_failure_safety",
                        )
                        .await
                        .unwrap_or(false);
                        if stop_sent {
                            let _ = request_embedded_audio_stop_feedback(
                                inner,
                                "provider_delivery_failure_safety",
                            );
                            if let Some(session) = self.session.as_mut() {
                                session.proactive_stop_dispatched = true;
                            }
                            log::info!(
                                "[coord] embedded audio proactive trailing-silence stop sent (session_id={}, silence_ms>={})",
                                chunk.session_id,
                                proactive_stop_silence_threshold_ms(
                                    self.session
                                        .as_ref()
                                        .and_then(|session| session.volcengine_asr.as_ref())
                                        .is_some_and(|asr| asr.audio_delivery_failed()),
                                )
                            );
                        }
                    }
                } else {
                    log::info!(
                        "[coord] embedded audio streaming tail packet excluded from ASR after STOP (session_id={}, packet_sequence={}, pcm_bytes={})",
                        chunk.session_id,
                        chunk.packet_sequence,
                        chunk.pcm.len()
                    );
                    self.show_transcribing_after_stop(inner);
                }
                if let Some(expected_packet_count) = self.pending_stop_expected_packet_count {
                    if self.collector.inner().has_successful_complete_session() {
                        self.finish_completed_streaming_session(
                            inner,
                            chunk_session_id,
                            expected_packet_count,
                        )
                        .await?;
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            crate::embedded_audio::StreamingSessionEvent::Stopped {
                session_id,
                expected_packet_count,
                origin,
            } => {
                // Mid-reopen / orphan VoiceActivation streams often deliver STOP with no
                // host session and no wake candidate. Finishing that path used to enter
                // the full dictation pipeline (or error-and-kill notify). Reset and keep
                // TYPE:READY so heartbeats are not starved by a dead-end finish.
                if self.session.is_none() && self.speaker_candidate.is_none() {
                    log::info!(
                        "[coord] embedded audio stop without active host session or wake candidate embedded_session_id={session_id}; resetting stream state and keeping notify open"
                    );
                    self.reset_for_next_session();
                    return Ok(true);
                }
                // A request-tagged marker resolves the ENSURE before this STOP.
                // In that case the same hidden segment was promoted and this
                // STOP is its real terminal boundary, not a request for a
                // second segment.  If a different segment was bound, the old
                // predecessor STOP is stale and must be ignored.
                let ensure_confirmed_same_segment = self
                    .accepted_wake_capture_ensure
                    .is_some_and(|ensure| {
                        ensure.confirmed_segment_id == Some(session_id)
                            && ensure.previous_segment_id == session_id
                    });
                let ensure_confirmed_replacement = self
                    .accepted_wake_capture_ensure
                    .is_some_and(|ensure| {
                        ensure.confirmed_segment_id.is_some_and(|actual| {
                            actual != ensure.previous_segment_id && session_id == ensure.previous_segment_id
                        })
                    });
                if ensure_confirmed_replacement {
                    log::info!(
                        "[wake-phrase] ignoring predecessor STOP after ENSURE continuation bound embedded_session_id={session_id}"
                    );
                    return Ok(false);
                }
                // 2026-08-09 12:46:59 激活竞态：旧唤醒段在 ENSURE 后 0.1s complete，
                // 只含 1845ms 唤醒词——此时 finalize 必空稿。竞态窗口内且正文未开始
                // 时，旧段 STOP 视为段 rotation：不 finalize，绑定激活后的新设备段.
                if !ensure_confirmed_same_segment {
                    if let Some((pre_segment_id, activated_at)) = self.activation_segment_race_guard {
                        if self.session.is_some() && session_id == pre_segment_id {
                        let body_started = self
                            .session
                            .as_ref()
                            .map(|session| session.session_id)
                            .is_some_and(|coordinator_session_id| {
                                automatic_wake_body_started(inner, coordinator_session_id)
                                    || current_embedded_audio_partial_preview(inner)
                                        .as_deref()
                                        .is_some_and(|text| !text.trim().is_empty())
                            });
                        let hidden_timeout = origin
                            == crate::embedded_audio::SessionStopOrigin::VoiceActivationMaxDuration;
                        let automatic_stop = matches!(origin,
                            crate::embedded_audio::SessionStopOrigin::VoiceActivation
                                | crate::embedded_audio::SessionStopOrigin::VoiceActivationMaxDuration);
                        if automatic_stop && (!body_started || hidden_timeout)
                            && !embedded_audio_stop_feedback_latched(inner)
                            && activated_at.elapsed() <= EMBEDDED_ACTIVATION_SEGMENT_RACE_WINDOW
                        {
                            log::info!(
                                "[coord] pre-activation embedded segment {session_id} stopped {}ms after wake activation origin={origin:?} body_started={body_started}; dictation session stays open for the post-activation segment",
                                activated_at.elapsed().as_millis()
                            );
                            if let Some(coordinator_session_id) =
                                self.session.as_ref().map(|session| session.session_id)
                            {
                                if let Some(ensure) = self.accepted_wake_capture_ensure.as_mut() {
                                    ensure.replacement_wait_started_at = Some(Instant::now());
                                    log::info!(
                                        "[wake-phrase] accepted wake capture predecessor stopped; awaiting ENSURE continuation marker request_id={} previous_segment_id={} embedded_session_id={session_id}",
                                        ensure.request_id,
                                        ensure.previous_segment_id
                                    );
                                }
                                mark_embedded_ble_awaiting_post_activation_segment(
                                    inner,
                                    coordinator_session_id,
                                );
                            }
                            self.collector.reset();
                            self.embedded_session_id = None;
                            self.pending_stop_expected_packet_count = None;
                            // This STOP completed only the pre-activation wake
                            // segment, not the live dictation. `true` tells the
                            // continuous actor that the whole pipeline is done;
                            // it then calls submission_result(), sees
                            // terminal_received=false, and discards the session
                            // as "尚未收到结束包". Keep the actor pending until the
                            // post-activation segment arrives.
                            return Ok(false);
                        }
                    }
                    // 窗口外或正文已开始：正常 finalize，竞态守卫使命结束。
                    self.activation_segment_race_guard = None;
                }
                }
                if let Some(session) = self.session.as_ref() {
                    crate::observability::record_embedded_audio_stop(session.session_id);
                }
                self.pending_stop_expected_packet_count = Some(expected_packet_count);
                self.show_transcribing_after_stop(inner);
                if self.collector.inner().has_successful_complete_session() {
                    self.pending_stop_force_after = None;
                    self.finish_completed_streaming_session(
                        inner,
                        session_id,
                        expected_packet_count,
                    )
                    .await?;
                    Ok(true)
                } else {
                    let stats = self.collector.inner().stats();
                    log::info!(
                        "[coord] embedded audio streaming stop received; waiting for tail packets (expected={}, received={}, missing={})",
                        expected_packet_count,
                        stats.received_packet_count,
                        stats.missing_packet_count
                    );
                    // Continuous notify must not wait forever for missing packets —
                    // capture stop-drain is 1s and now keeps the link open; force
                    // finish slightly after that so TYPE:READY is not starved.
                    if self.keep_listening_after_pipeline_errors {
                        self.pending_stop_force_after =
                            Some(Instant::now() + Duration::from_millis(1_200));
                    }
                    Ok(false)
                }
            }
            crate::embedded_audio::StreamingSessionEvent::Cancelled { session_id, .. } => {
                if self.session.is_none() {
                    if let Some(mut candidate) = self.speaker_candidate.take() {
                        let candidate_observation = self.pipeline_observation.clone();
                        let discarded_bytes = candidate.pcm.len();
                        candidate.record_outcome_fact(
                            candidate_observation.as_ref(),
                            crate::observability::CandidateFactKind::Discarded,
                            "candidate_cancelled",
                            discarded_bytes,
                        );
                        candidate.record_outcome_fact(
                            candidate_observation.as_ref(),
                            crate::observability::CandidateFactKind::Closed,
                            "candidate_cancelled",
                            0,
                        );
                        self.preserve_candidate_fact_ledger(&mut candidate);
                        if candidate.kind == BufferedSpeakerCandidateKind::Enrollment {
                            crate::speaker_verification::fail_enrollment("嵌入式音频会话已取消");
                            complete_voiceprint_enrollment_candidate(
                                "voiceprint_enrollment_cancelled",
                            );
                        } else {
                            reject_hidden_automatic_candidate(
                                inner,
                                "hidden_candidate_cancelled",
                                session_id,
                                HiddenCandidateTransportState::Ended,
                            );
                        }
                        self.reset_for_next_session();
                        // A terminal device CANCEL ends only this logical candidate.
                        // The continuous actor must keep waiting on the same notify
                        // subscription; returning `true` makes it call
                        // submission_result() after reset and manufacture a
                        // misleading "尚未收到结束包" soft error.
                        return Ok(!self.keep_listening_after_pipeline_errors);
                    }
                    // No host session and no candidate: keep notify (continuous path).
                    log::info!(
                        "[coord] embedded audio cancel without active stream work embedded_session_id={session_id}; keeping notify open"
                    );
                    self.reset_for_next_session();
                    return Ok(!self.keep_listening_after_pipeline_errors);
                }
                if self.keep_listening_after_pipeline_errors {
                    self.discard_active_session_after_user_cancel(inner);
                    return Ok(false);
                }
                self.abort_streaming_session(inner, session_id, "嵌入式音频会话已取消");
                Err("嵌入式音频会话已取消".to_string())
            }
            crate::embedded_audio::StreamingSessionEvent::Error {
                session_id,
                error_code,
                ..
            } => {
                let message = format!("嵌入式音频会话错误: {error_code:?}");
                if self.keep_listening_after_pipeline_errors {
                    self.discard_active_session_after_stream_error(inner, &message);
                    // State is already reset and the error has already been
                    // recorded once. Keep the continuous actor pending instead
                    // of asking it to build a submission from empty state.
                    return Ok(false);
                }
                self.abort_streaming_session(inner, session_id, &message);
                Err(message)
            }
            crate::embedded_audio::StreamingSessionEvent::Ignored(reason) => {
                log::debug!("[coord] embedded audio streaming ignored packet: {reason:?}");
                Ok(false)
            }
        }
    }

}

include!("dictation_embedded_candidate_begin.rs");

impl EmbeddedStreamingDictation {

    async fn finish_streaming_session(
        &mut self,
        inner: &Arc<Inner>,
        embedded_session_id: u32,
        expected_packet_count: u16,
    ) -> Result<(), String> {
        if self.embedded_session_id != Some(embedded_session_id) {
            return Err(format!(
                "嵌入式音频停止包 session 不一致: current={:?}, incoming={embedded_session_id}",
                self.embedded_session_id
            ));
        }
        let stats = self.collector.inner().stats();
        if stats.received_pcm_bytes == 0 {
            self.abort_streaming_session(
                inner,
                embedded_session_id,
                "嵌入式音频会话没有可识别的 PCM 数据",
            );
            return Err("嵌入式音频会话没有可识别的 PCM 数据".to_string());
        }
        if stats.missing_packet_count > 0 {
            log::warn!(
                "[coord] embedded audio streaming stop with missing packets (expected={}, missing={:?})",
                expected_packet_count,
                stats.missing_packet_indices
            );
        }
        if stats.post_stop_packet_count > 0 {
            log::info!(
                "[coord] embedded audio streaming collected post-stop tail for diagnostics (tail_packets={}, tail_pcm_bytes={}, tail_duration={:.3}s, asr_pcm_bytes={})",
                stats.post_stop_packet_count,
                stats.post_stop_pcm_bytes,
                stats.post_stop_duration_seconds,
                stats.asr_boundary_pcm_bytes
            );
        }
        store_embedded_audio_stats(inner, stats.clone());
        self.promote_hidden_candidate_if_requested(inner, embedded_session_id)
            .await?;
        if self.speaker_candidate.is_some()
            && self
                .finish_buffered_speaker_candidate(inner, embedded_session_id)
                .await?
        {
            // A terminal wake accepted after its physical segment ended keeps
            // the exact candidate identity until the requested continuation
            // segment attaches and promotes it. Other terminal outcomes close
            // the candidate here.
            if terminal_wake_continuation_waiting_for_audio(inner).is_none() {
                let _ = inner
                    .recording_lifecycle
                    .lock()
                    .close_candidate(embedded_session_id);
            }
            return Ok(());
        }
        self.show_transcribing_after_stop(inner);

        let mut session = self
            .session
            .take()
            .ok_or_else(|| "嵌入式音频流式听写 session 尚未创建".to_string())?;
        self.preserve_session_candidate_fact_ledger(&mut session);
        // STOP ends device delivery, so commit the final partial provider block before
        // finalizing ASR. This keeps the final syllables in the authoritative stream.
        session.flush_streaming_pcm();
        let archive_active = session
            .archive_pcm
            .as_deref()
            .map(|pcm| archive_embedded_audio_if_enabled(inner, session.session_id, pcm))
            .unwrap_or(false);
        inner
            .audio_archive_active
            .store(archive_active, std::sync::atomic::Ordering::Relaxed);
        if session.active_asr == "volcengine" {
            log::info!(
                "[coord] embedded audio level summary (mode=firmware_afe_agc_host_attenuation_limiter, first_voiced_pcm_ms={:?}, voiced_chunks={}, quiet_chunks={}, observed_signal_rms_min={:?}, observed_signal_rms_max={:.2}, observed_signal_peak_max={}, pre_calibration_quiet_chunks={}, pre_calibration_signal_rms_max={:.2}, pre_calibration_signal_peak_max={}, first_eligible_signal_rms={:?}, first_eligible_signal_peak={:?}, first_gain={:?}, final_gain={:.4}, max_gain={:.4}, limiter_blocks={}, limiter_reduction_db_max={:.2}, upstream_clipped_samples={}, newly_clipped_samples={})",
                session.streaming_agc.first_voiced_pcm_ms,
                session.streaming_agc.voiced_chunks,
                session.streaming_agc.quiet_chunks,
                session.streaming_agc.observed_signal_rms_min,
                session.streaming_agc.observed_signal_rms_max,
                session.streaming_agc.observed_signal_peak_max,
                session.streaming_agc.pre_calibration_quiet_chunks,
                session.streaming_agc.pre_calibration_signal_rms_max,
                session.streaming_agc.pre_calibration_signal_peak_max,
                session.streaming_agc.first_eligible_signal_rms,
                session.streaming_agc.first_eligible_signal_peak,
                session.streaming_agc.first_gain,
                session.streaming_agc.gain,
                session.streaming_agc.max_gain,
                session.streaming_agc.gain_update_count,
                session.streaming_agc.limiter_reduction_db_max,
                session.streaming_agc.upstream_clipped_samples,
                session.streaming_agc.clipped_samples
            );
        }
        log::info!(
            "[coord] embedded audio streaming submitted to dictation pipeline (asr={}, input_mode={}, pcm_bytes={}, asr_pcm_bytes={}, archive={})",
            session.active_asr,
            if session.active_asr == "volcengine" {
                "firmware_afe_agc_host_attenuation_limiter"
            } else {
                "host_attenuation_limiter"
            },
            session.streamed_pcm_bytes,
            session.normalized_pcm_bytes,
            archive_active
        );
        let coordinator_session_id = session.session_id;
        let user_initiated_stop =
            embedded_audio_stop_is_user_initiated(self.collector.inner().stats().stop_origin);
        let end_result = end_embedded_ble_session_with_source_integrity(
            inner,
            user_initiated_stop,
            format!(
                "embedded_session_id={embedded_session_id} coordinator_session_id={} expected_packets={expected_packet_count}",
                coordinator_session_id
            ),
            Some(Arc::clone(&session.source_admission_ledger)),
        )
        .await;
        if end_result.is_ok() {
            self.transcript = take_embedded_audio_final_result(inner, coordinator_session_id);
        }
        // Keep the provider timeline for failed finalization as well.  The WAV is
        // deliberately archived before this point, so an empty/error result is
        // exactly the case where the trace is most useful for separating device
        // delivery, provider recognition, and product arbitration.  This is
        // observation-only: it does not change the final-result or stop path.
        if archive_active && record_embedded_audio_for_debug_enabled(inner) {
            if let Some(asr) = session.volcengine_asr.as_ref() {
                let trace = asr.take_diagnostic_trace();
                if let (Ok(wav), Ok(path)) = (
                    crate::persistence::recording_path_for_session(&coordinator_session_id.to_string()),
                    crate::persistence::asr_trace_path_for_session(&coordinator_session_id.to_string()),
                ) {
                    if wav.exists() {
                        let final_text = self.transcript.as_ref().map(|t| t.final_text.as_str());
                        let payload = serde_json::json!({
                            "sessionId": coordinator_session_id.to_string(),
                            "finalCoordinatorText": final_text,
                            "finalCoordinatorTextAvailable": final_text.is_some(),
                            "finalizationSucceeded": end_result.is_ok(),
                            "finalizationError": end_result.as_ref().err().cloned(),
                            "userInitiatedStop": user_initiated_stop,
                            "streamedPcmBytes": session.streamed_pcm_bytes,
                            "normalizedPcmBytes": session.normalized_pcm_bytes,
                            "archivePcmBytes": session.archive_pcm.as_ref().map_or(0, Vec::len),
                            "audioDelivery": asr.diagnostic_audio_delivery(),
                            "trace": trace,
                        });
                        if let Ok(bytes) = serde_json::to_vec(&payload) {
                            if let Err(err) = std::fs::write(path, bytes) {
                                log::warn!("[coord] ASR diagnostic archive failed: {err}");
                            }
                        }
                    }
                }
            }
        }
        if end_result.is_ok() {
            let _ = inner
                .recording_lifecycle
                .lock()
                .close_owner(coordinator_session_id);
        }
        end_result
    }

}

include!("dictation_embedded_detector_init.rs");

impl EmbeddedStreamingDictation {

    async fn finish_buffered_speaker_candidate(
        &mut self,
        inner: &Arc<Inner>,
        embedded_session_id: u32,
    ) -> Result<bool, String> {
        let Some(mut candidate) = self.speaker_candidate.take() else {
            return Ok(false);
        };
        if candidate.kind == BufferedSpeakerCandidateKind::Rejected {
            candidate.record_outcome_fact(
                self.pipeline_observation.as_ref(),
                crate::observability::CandidateFactKind::Closed,
                "candidate_rejected",
                0,
            );
            if let Some(sid) = take_early_capsule_session_id(&mut candidate) {
                dismiss_early_wake_recording_capsule(inner, sid);
            }
            self.preserve_candidate_fact_ledger(&mut candidate);
            reject_hidden_automatic_candidate(
                inner,
                "automatic_candidate_rejected",
                embedded_session_id,
                HiddenCandidateTransportState::Ended,
            );
            return Ok(true);
        }
        if candidate.kind == BufferedSpeakerCandidateKind::Enrollment {
            if !crate::speaker_verification::enrollment_should_process() {
                log::info!("[speaker-verification] discarded cancelled enrollment candidate embedded_session_id={embedded_session_id}");
                let discarded_bytes = candidate.pcm.len();
                candidate.record_outcome_fact(
                    self.pipeline_observation.as_ref(),
                    crate::observability::CandidateFactKind::Discarded,
                    "voiceprint_enrollment_cancelled",
                    discarded_bytes,
                );
                candidate.record_outcome_fact(
                    self.pipeline_observation.as_ref(),
                    crate::observability::CandidateFactKind::Closed,
                    "voiceprint_enrollment_cancelled",
                    0,
                );
                self.preserve_candidate_fact_ledger(&mut candidate);
                complete_voiceprint_enrollment_candidate("voiceprint_enrollment_cancelled");
                return Ok(true);
            }
            let pcm = std::mem::take(&mut candidate.pcm);
            let phrase = inner.prefs.get().voice_wake_phrase;
            crate::speaker_verification::begin_enrollment_processing();
            let result = tauri::async_runtime::spawn_blocking(move || {
                crate::speaker_verification::finish_enrollment(&pcm, &phrase)
            })
            .await
            .map_err(|err| format!("声纹登记处理任务失败: {err}"))
            .and_then(|result| result);
            let status = match result {
                Ok(status) => status,
                Err(err) => {
                    crate::speaker_verification::fail_enrollment(&err);
                    log::warn!(
                        "[speaker-verification] enrollment failed embedded_session_id={embedded_session_id}: {err}"
                    );
                    candidate.record_outcome_fact(
                        self.pipeline_observation.as_ref(),
                        crate::observability::CandidateFactKind::Rejected,
                        "voiceprint_enrollment_failed",
                        0,
                    );
                    candidate.record_outcome_fact(
                        self.pipeline_observation.as_ref(),
                        crate::observability::CandidateFactKind::Closed,
                        "voiceprint_enrollment_failed",
                        0,
                    );
                    self.preserve_candidate_fact_ledger(&mut candidate);
                    complete_voiceprint_enrollment_candidate("voiceprint_enrollment_failed");
                    return Ok(true);
                }
            };
            log::info!(
                "[speaker-verification] enrollment complete embedded_session_id={} enrolled={} score={:?}",
                embedded_session_id,
                status.enrolled,
                status.score
            );
            #[cfg(target_os = "windows")]
            {
                tauri::async_runtime::spawn_blocking(move || {
                    match crate::asr::local::wake_helper::preload() {
                        Ok(()) => log::info!(
                            "[wake-phrase] isolated local confirmation helper prepared after enrollment"
                        ),
                        Err(err) => log::info!(
                            "[wake-phrase] isolated local confirmation helper remains unavailable after enrollment: {err}"
                        ),
                    }
                });
            }
            candidate.record_outcome_fact(
                self.pipeline_observation.as_ref(),
                crate::observability::CandidateFactKind::Closed,
                "voiceprint_enrollment_complete",
                0,
            );
            self.preserve_candidate_fact_ledger(&mut candidate);
            complete_voiceprint_enrollment_candidate("voiceprint_enrollment_complete");
            return Ok(true);
        }

        let automatic = candidate.kind == BufferedSpeakerCandidateKind::Verification;
        let mut local_speaker_seed = None;
        if automatic {
            let phrase = inner.prefs.get().voice_wake_phrase;
            // Finish deferred detector init before terminal KWS/offline pass.
            if candidate.wake_detector.is_none() {
                if let Some(handle) = candidate.wake_detector_init.take() {
                    match handle.await {
                        Ok(Ok(detector)) => {
                            candidate.wake_detector = Some(detector);
                            log::info!(
                                "[wake-phrase] deferred streaming detector ready at terminal embedded_session_id={embedded_session_id}"
                            );
                        }
                        Ok(Err(err)) => {
                            log::warn!(
                                "[wake-phrase] deferred streaming detector init failed embedded_session_id={embedded_session_id}: {err}"
                            );
                        }
                        Err(err) => {
                            log::warn!(
                                "[wake-phrase] deferred streaming detector task failed embedded_session_id={embedded_session_id}: {err}"
                            );
                        }
                    }
                }
            }
            // STOP can arrive a few milliseconds after the final rolling window
            // schedules its local confirmation. Dropping that already-running
            // task made late wake phrases deterministically lose their last
            // evidence, while the blocking helper continued in the background.
            // Wait only the remainder of the fixed budget already charged from
            // task start; do not launch any additional work here.
            #[cfg(target_os = "windows")]
            let mut terminal_completed_local_confirmation = None;
            #[cfg(target_os = "windows")]
            if let Some(task) = candidate.local_confirmation_task.take() {
                let task_origin_bytes = candidate.local_confirmation_task_origin_bytes;
                let task_has_keyword_model_hit =
                    candidate.local_confirmation_task_has_keyword_model_hit;
                let elapsed_ms = candidate
                    .local_confirmation_task_started_at
                    .take()
                    .map(|started| started.elapsed().as_millis() as u64)
                    .unwrap_or(0);
                let remaining_ms = terminal_inflight_confirmation_remaining_ms(elapsed_ms);
                let already_finished = task.inner().is_finished();
                log::info!(
                    "[wake-phrase] terminal joining in-flight local confirmation embedded_session_id={} elapsed_ms={} remaining_ms={} already_finished={} window_origin_pcm_ms={} kws_hit={}",
                    embedded_session_id,
                    elapsed_ms,
                    remaining_ms,
                    already_finished,
                    task_origin_bytes / 32,
                    task_has_keyword_model_hit
                );
                let outcome = if already_finished {
                    Some(task.await)
                } else if remaining_ms == 0 {
                    None
                } else {
                    match tokio::time::timeout(Duration::from_millis(remaining_ms), task).await {
                        Ok(result) => Some(result),
                        Err(_) => None,
                    }
                };
                match outcome {
                    Some(Ok(Ok(result))) => {
                        log::info!(
                            "[wake-phrase] terminal in-flight local confirmation finished embedded_session_id={} matched={} phrase_relation={:?} snapshot_pcm_ms={} transcript_chars={} phonetic_prefix_units={} phonetic_best_distance={} phonetic_best_window_start={} inference_ms={} window_origin_pcm_ms={} kws_hit={}",
                            embedded_session_id,
                            result.matched,
                            result.phrase_relation,
                            result.snapshot_pcm_ms,
                            result.transcript_chars,
                            result.phonetic_prefix_units,
                            result.phonetic_best_distance,
                            result.phonetic_best_window_start,
                            result.inference_ms,
                            task_origin_bytes / 32,
                            task_has_keyword_model_hit
                        );
                        terminal_completed_local_confirmation = Some((
                            result,
                            task_origin_bytes,
                            task_has_keyword_model_hit,
                        ));
                    }
                    Some(Ok(Err(err))) => log::info!(
                        "[wake-phrase] terminal in-flight local confirmation unavailable embedded_session_id={embedded_session_id}: {err}"
                    ),
                    Some(Err(err)) => log::warn!(
                        "[wake-phrase] terminal in-flight local confirmation task failed embedded_session_id={embedded_session_id}: {err}"
                    ),
                    None => log::info!(
                        "[wake-phrase] terminal in-flight local confirmation exceeded remaining budget embedded_session_id={} elapsed_ms={} remaining_ms={}",
                        embedded_session_id,
                        elapsed_ms,
                        remaining_ms
                    ),
                }
            }
            let Some(mut detector) = candidate.wake_detector.take() else {
                if let Some(sid) = take_early_capsule_session_id(&mut candidate) {
                    dismiss_early_wake_recording_capsule(inner, sid);
                }
                save_bounded_wake_diagnostic(
                    embedded_session_id,
                    "detector-unavailable",
                    &candidate.pcm,
                );
                let rejected_bytes = candidate.pcm.len();
                candidate.record_outcome_fact(
                    self.pipeline_observation.as_ref(),
                    crate::observability::CandidateFactKind::Rejected,
                    "wake_phrase_detection_failed",
                    rejected_bytes,
                );
                candidate.record_outcome_fact(
                    self.pipeline_observation.as_ref(),
                    crate::observability::CandidateFactKind::Closed,
                    "wake_phrase_detection_failed",
                    0,
                );
                self.preserve_candidate_fact_ledger(&mut candidate);
                reject_hidden_automatic_candidate(
                    inner,
                    "wake_phrase_detection_failed",
                    embedded_session_id,
                    HiddenCandidateTransportState::Ended,
                );
                return Ok(true);
            };
            let terminal_feed_start = candidate.kws_fed_bytes;
            let remaining_pcm = candidate.pcm[terminal_feed_start..].to_vec();
            let terminal_feed_bytes = remaining_pcm.len();
            let terminal_kws_feed_range = terminal_feed_start
                .checked_add(terminal_feed_bytes)
                .filter(|end| *end <= candidate.pcm.len())
                .and_then(|_| {
                    candidate_slice_range(
                        candidate.pcm_base_offset,
                        terminal_feed_start,
                        terminal_feed_bytes,
                    )
                });
            let terminal_kws_source_runs =
                candidate.source_facts_for_range(terminal_kws_feed_range);
            let stream_origin_bytes = candidate.kws_stream_origin_bytes;
            let wake_task = tauri::async_runtime::spawn_blocking(move || {
                let started = Instant::now();
                let result = detector
                    .accept_pcm(&remaining_pcm)
                    .and_then(|found| match found {
                        Some(found) => Ok(Some(found)),
                        None => detector.finish(),
                    });
                (detector, result, started.elapsed().as_millis() as u64)
            })
            .await;
            let (_, wake_match, final_kws_ms) = match wake_task {
                Ok(result) => {
                    candidate.kws_fed_bytes = candidate.pcm.len();
                    candidate.record_fact(
                        self.pipeline_observation.as_ref(),
                        crate::observability::CandidateFactKind::KwsFed,
                        candidate.current_candidate_range(),
                        terminal_kws_feed_range,
                        None,
                        None,
                        None,
                        terminal_kws_source_runs,
                        terminal_feed_bytes,
                        Some("terminal_accept_pcm_feed"),
                    );
                    result
                }
                Err(err) => {
                    log::warn!(
                        "[wake-phrase] terminal streaming detector task failed embedded_session_id={embedded_session_id}: {err}"
                    );
                    candidate.kws_fed_bytes = terminal_feed_start;
                    candidate.record_fact(
                        self.pipeline_observation.as_ref(),
                        crate::observability::CandidateFactKind::KwsFeedUnknown,
                        candidate.current_candidate_range(),
                        terminal_kws_feed_range,
                        None,
                        None,
                        None,
                        candidate.source_facts_for_range(terminal_kws_feed_range),
                        terminal_feed_bytes,
                        Some("terminal_kws_task_join_unknown"),
                    );
                    let rejected_bytes = candidate.pcm.len();
                    candidate.record_outcome_fact(
                        self.pipeline_observation.as_ref(),
                        crate::observability::CandidateFactKind::Rejected,
                        "wake_phrase_detection_failed",
                        rejected_bytes,
                    );
                    candidate.record_outcome_fact(
                        self.pipeline_observation.as_ref(),
                        crate::observability::CandidateFactKind::Closed,
                        "wake_phrase_detection_failed",
                        0,
                    );
                    self.preserve_candidate_fact_ledger(&mut candidate);
                    reject_hidden_automatic_candidate(
                        inner,
                        "wake_phrase_detection_failed",
                        embedded_session_id,
                        HiddenCandidateTransportState::Ended,
                    );
                    return Ok(true);
                }
            };
            candidate.kws_total_ms = candidate.kws_total_ms.saturating_add(final_kws_ms);
            let (enrolled_owner_matched, verification, voiceprint_ms) =
                terminal_owner_verification_for_recall(&candidate.pcm, &phrase).await;
            let mut phrase_signal = denzic_voice_activation_v1_core::PhraseSignal::KeywordModel;
            let mut local_confirmation_ms = 0u64;
            #[cfg(target_os = "windows")]
            if let Some((result, _, _)) = terminal_completed_local_confirmation.as_ref() {
                local_confirmation_ms = local_confirmation_ms.saturating_add(result.inference_ms);
            }
            let mut wake_match = match wake_match.map(|found| {
                offset_streaming_wake_match(found, stream_origin_bytes)
            }) {
                Ok(Some(found)) => {
                    #[cfg(target_os = "windows")]
                    {
                        let confirm = spawn_local_wake_confirmation(
                            inner,
                            LocalWakeConfirmationDiagnosticContext {
                                embedded_session_id,
                                attempt: None,
                                source_origin_bytes: 0,
                                branch: "terminal-kws",
                            },
                            candidate.pcm.clone(),
                            phrase.clone(),
                            false,
                        )
                        .await;
                        match confirm {
                            Ok(Ok(result)) => {
                                local_confirmation_ms = local_confirmation_ms
                                    .saturating_add(result.inference_ms);
                                log::info!(
                                    "[wake-phrase] terminal KWS local confirmation finished embedded_session_id={} matched={} phrase_relation={:?} snapshot_pcm_ms={} transcript_chars={} phonetic_prefix_units={} phonetic_best_distance={} phonetic_best_window_start={} inference_ms={}",
                                    embedded_session_id,
                                    result.matched,
                                    result.phrase_relation,
                                    result.snapshot_pcm_ms,
                                    result.transcript_chars,
                                    result.phonetic_prefix_units,
                                    result.phonetic_best_distance,
                                    result.phonetic_best_window_start,
                                    result.inference_ms
                                );
                                if result.matched {
                                    phrase_signal =
                                        denzic_voice_activation_v1_core::PhraseSignal::LocalTranscript;
                                    Some(found)
                                } else if let Some(signal) =
                                    enrolled_terminal_kws_phonetic_fusion_signal(
                                    enrolled_owner_matched,
                                    &result,
                                    phrase.chars().count(),
                                    embedded_session_id,
                                )
                                {
                                    phrase_signal = signal;
                                    Some(found)
                                } else if !crate::speaker_verification::is_enrolled_for_phrase(
                                    &phrase,
                                ) {
                                    // Without a voiceprint, preserve KWS recall when
                                    // the short local-ASR confirmation is absent.
                                    log::info!(
                                        "[wake-phrase] terminal stage2 Absent fail-open KeywordModel open-gate embedded_session_id={} prior_absent_count={} transcript_chars={}",
                                        embedded_session_id,
                                        candidate.kws_local_absent_count,
                                        result.transcript_chars
                                    );
                                    phrase_signal =
                                        denzic_voice_activation_v1_core::PhraseSignal::KeywordModel;
                                    Some(found)
                                } else {
                                    log::info!(
                                        "[wake-phrase] terminal stage2 Absent reject KWS embedded_session_id={} prior_absent_count={} (anti false-wake enrolled)",
                                        embedded_session_id,
                                        candidate.kws_local_absent_count
                                    );
                                    None
                                }
                            }
                            Ok(Err(err)) => {
                                let explicit_absent_count =
                                    keyword_fallback_absent_count(&candidate, &found);
                                if secondary_fallback_can_accept_keyword(
                                    true,
                                    explicit_absent_count,
                                ) {
                                    log::warn!(
                                        "[wake-phrase] terminal stage2 unavailable embedded_session_id={embedded_session_id}: {err}; fail-open KeywordModel"
                                    );
                                    phrase_signal =
                                        denzic_voice_activation_v1_core::PhraseSignal::KeywordModel;
                                    Some(found)
                                } else {
                                    log::info!(
                                        "[wake-phrase] terminal stage2 unavailable held after explicit Absent embedded_session_id={} absent_count={}",
                                        embedded_session_id,
                                        explicit_absent_count
                                    );
                                    None
                                }
                            }
                            Err(err) => {
                                let explicit_absent_count =
                                    keyword_fallback_absent_count(&candidate, &found);
                                if secondary_fallback_can_accept_keyword(
                                    true,
                                    explicit_absent_count,
                                ) {
                                    log::warn!(
                                        "[wake-phrase] terminal stage2 task failed embedded_session_id={embedded_session_id}: {err}; fail-open KeywordModel"
                                    );
                                    phrase_signal =
                                        denzic_voice_activation_v1_core::PhraseSignal::KeywordModel;
                                    Some(found)
                                } else {
                                    log::info!(
                                        "[wake-phrase] terminal stage2 task failure held after explicit Absent embedded_session_id={} absent_count={}",
                                        embedded_session_id,
                                        explicit_absent_count
                                    );
                                    None
                                }
                            }
                        }
                    }
                    #[cfg(not(target_os = "windows"))]
                    {
                        Some(found)
                    }
                }
                Ok(None) => {
                    // Streaming KWS missed — still try offline full-buffer cascade.
                    // Do not skip on midstream local Absent: that dropped last-chance
                    // recall when streaming never hit (owner quiet/device VA).
                    #[cfg(target_os = "windows")]
                    let terminal_local_match = terminal_completed_local_confirmation
                        .take()
                        .and_then(|(result, task_origin_bytes, task_has_keyword_model_hit)| {
                            if enrolled_terminal_local_near_can_accept(
                                enrolled_owner_matched,
                                &result,
                                phrase.chars().count(),
                                task_origin_bytes,
                            ) {
                                phrase_signal = denzic_voice_activation_v1_core::PhraseSignal::LocalTranscript;
                                log::info!(
                                    "[wake-phrase] terminal enrolled owner recovered start-aligned local near-match embedded_session_id={} prefix_units={} distance={} transcript_chars={}",
                                    embedded_session_id,
                                    result.phonetic_prefix_units,
                                    result.phonetic_best_distance,
                                    result.transcript_chars
                                );
                                return Some(crate::wake_phrase::Match {
                                    start_seconds: None,
                                    end_seconds: LOCAL_ONLY_START_ENDPOINT_MAX_SECONDS,
                                    matched_keyword: None,
                                });
                            }
                            if open_terminal_local_near_can_accept(
                                &verification,
                                &result,
                                phrase.chars().count(),
                                task_origin_bytes,
                            ) {
                                phrase_signal = denzic_voice_activation_v1_core::PhraseSignal::LocalTranscript;
                                log::info!(
                                    "[wake-phrase] terminal open near-phrase wake accepted (voiceprint floor) embedded_session_id={} prefix_units={} distance={} window_start={} transcript_chars={} owner_score={:.6}",
                                    embedded_session_id,
                                    result.phonetic_prefix_units,
                                    result.phonetic_best_distance,
                                    result.phonetic_best_window_start,
                                    result.transcript_chars,
                                    verification.as_ref().map(|result| result.score).unwrap_or_default()
                                );
                                return Some(crate::wake_phrase::Match {
                                    start_seconds: None,
                                    end_seconds: LOCAL_ONLY_START_ENDPOINT_MAX_SECONDS,
                                    matched_keyword: None,
                                });
                            }
                            match terminal_inflight_local_decision(
                                &result,
                                task_has_keyword_model_hit,
                                phrase.chars().count(),
                            ) {
                                TerminalInflightLocalDecision::AcceptLocal => {
                                    let refined_end = refined_wake_end_seconds(
                                        0.0,
                                        &result,
                                        phrase.chars().count(),
                                    );
                                    let refined_end = if task_has_keyword_model_hit {
                                        refined_end
                                    } else {
                                        refined_end + task_origin_bytes as f32 / 32_000.0
                                    };
                                    phrase_signal = denzic_voice_activation_v1_core::PhraseSignal::LocalTranscript;
                                    Some(crate::wake_phrase::Match {
                                        start_seconds: None,
                                        end_seconds: refined_end,
                                        matched_keyword: None,
                                    })
                                }
                                TerminalInflightLocalDecision::PreserveKwsFusion => {
                                    candidate.local_kws_fusion_evidence = true;
                                    None
                                }
                                TerminalInflightLocalDecision::RecordAbsent => {
                                    candidate.local_absent_count =
                                        candidate.local_absent_count.saturating_add(1);
                                    if let Some(coverage) = authoritative_local_absent_coverage(
                                        result.phrase_relation,
                                        result.transcript_chars,
                                        phrase.chars().count(),
                                        task_origin_bytes,
                                        task_origin_bytes.saturating_add(
                                            result.snapshot_pcm_ms.saturating_mul(32),
                                        ),
                                    ) {
                                        candidate.local_absent_coverage = Some(coverage);
                                    }
                                    None
                                }
                            }
                        });
                    #[cfg(not(target_os = "windows"))]
                    let terminal_local_match: Option<crate::wake_phrase::Match> = None;

                    if terminal_local_match.is_some() {
                        terminal_local_match
                    } else if !should_run_terminal_offline_recall(
                        candidate.pcm.len(),
                        candidate.local_absent_count,
                        candidate.local_kws_fusion_evidence,
                        enrolled_owner_matched,
                    ) {
                        let too_short =
                            candidate.pcm.len() < MIN_TERMINAL_OFFLINE_PCM_BYTES;
                        log::info!(
                            "[wake-phrase] terminal skip offline cascade reason={} embedded_session_id={} pcm_ms={} min_ms={} local_absent_count={} enrolled_owner_matched={}",
                            if too_short { "candidate_too_short" } else { "local_absent_evidence" },
                            embedded_session_id,
                            candidate.pcm.len() / 32,
                            MIN_TERMINAL_OFFLINE_PCM_BYTES / 32,
                            candidate.local_absent_count,
                            enrolled_owner_matched
                        );
                        None
                    } else {
                        // Offline full-buffer gain + bounded cascade may recover gain-starved
                        // device candidates — but only as a provisional KeywordModel. Local
                        // transcript must still confirm the full wake phrase.
                        let offline_pcm = candidate.pcm.clone();
                        let offline_phrase = phrase.clone();
                        let offline_task = tauri::async_runtime::spawn_blocking(move || {
                            crate::wake_phrase::detect_with_recall_cascade_bounded(
                                &offline_pcm,
                                &offline_phrase,
                                Duration::from_millis(TERMINAL_OFFLINE_RECALL_BUDGET_MS),
                            )
                        });
                        let offline = match tokio::time::timeout(
                            Duration::from_millis(TERMINAL_OFFLINE_RECALL_BUDGET_MS),
                            offline_task,
                        )
                        .await
                        {
                            Ok(Ok(result)) => result,
                            Ok(Err(err)) => {
                                log::warn!(
                                    "[wake-phrase] terminal offline recall task failed embedded_session_id={embedded_session_id}: {err}"
                                );
                                Ok(None)
                            }
                            Err(_) => {
                                log::info!(
                                    "[wake-phrase] terminal offline recall released actor after bounded wait embedded_session_id={} budget_ms={}",
                                    embedded_session_id,
                                    TERMINAL_OFFLINE_RECALL_BUDGET_MS
                                );
                                Ok(None)
                            }
                        };
                        match offline {
                            Ok(Some(found)) => {
                                log::info!(
                                    "[wake-phrase] terminal offline recall provisional hit embedded_session_id={} end_s={:.3}; requiring local full-phrase confirm",
                                    embedded_session_id,
                                    found.end_seconds
                                );
                                #[cfg(target_os = "windows")]
                                {
                                    let (confirm_pcm, confirm_origin) =
                                        terminal_local_confirmation_pcm(&candidate.pcm);
                                    let confirm = spawn_local_wake_confirmation(
                                        inner,
                                        LocalWakeConfirmationDiagnosticContext {
                                            embedded_session_id,
                                            attempt: None,
                                            source_origin_bytes: confirm_origin,
                                            branch: "terminal-offline",
                                        },
                                        confirm_pcm,
                                        phrase.clone(),
                                        false,
                                    )
                                    .await;
                                    match confirm {
                                        Ok(Ok(result)) => {
                                            local_confirmation_ms = local_confirmation_ms
                                                .saturating_add(result.inference_ms);
                                            log::info!(
                                                "[wake-phrase] terminal offline→local confirmation finished embedded_session_id={} matched={} phrase_relation={:?} snapshot_pcm_ms={} transcript_chars={} phonetic_prefix_units={} phonetic_best_distance={} phonetic_best_window_start={} inference_ms={}",
                                                embedded_session_id,
                                                result.matched,
                                                result.phrase_relation,
                                                result.snapshot_pcm_ms,
                                                result.transcript_chars,
                                                result.phonetic_prefix_units,
                                                result.phonetic_best_distance,
                                                result.phonetic_best_window_start,
                                                result.inference_ms
                                            );
                                            let phonetic_near =
                                                phonetic_near_phrase_evidence(
                                                    &result,
                                                    phrase.chars().count(),
                                                );
                                            if result.matched || phonetic_near {
                                                phrase_signal = if result.matched {
                                                    denzic_voice_activation_v1_core::PhraseSignal::LocalTranscript
                                                } else {
                                                    log::info!(
                                                        "[wake-phrase] terminal offline KWS fused with one-unit phonetic near-match embedded_session_id={} phonetic_best_distance={} transcript_chars={}",
                                                        embedded_session_id,
                                                        result.phonetic_best_distance,
                                                        result.transcript_chars
                                                    );
                                                    denzic_voice_activation_v1_core::PhraseSignal::KeywordModel
                                                };
                                                Some(found)
                                            } else {
                                                // Offline cascade is a weak stage-1; Absent rejects.
                                                log::info!(
                                                    "[wake-phrase] terminal offline stage2 Absent reject embedded_session_id={} (anti false-wake)",
                                                    embedded_session_id
                                                );
                                                None
                                            }
                                        }
                                        Ok(Err(err)) => {
                                            log::warn!(
                                                "[wake-phrase] terminal offline stage2 unavailable embedded_session_id={embedded_session_id}: {err}; fail-open KeywordModel"
                                            );
                                            phrase_signal = denzic_voice_activation_v1_core::PhraseSignal::KeywordModel;
                                            Some(found)
                                        }
                                        Err(err) => {
                                            log::warn!(
                                                "[wake-phrase] terminal offline stage2 task failed embedded_session_id={embedded_session_id}: {err}; fail-open KeywordModel"
                                            );
                                            phrase_signal = denzic_voice_activation_v1_core::PhraseSignal::KeywordModel;
                                            Some(found)
                                        }
                                    }
                                }
                                #[cfg(not(target_os = "windows"))]
                                {
                                    Some(found)
                                }
                            }
                            Ok(None) | Err(_) => {
                                if let Err(err) = &offline {
                                    log::warn!(
                                        "[wake-phrase] terminal offline recall failed embedded_session_id={embedded_session_id}: {err}"
                                    );
                                }
                                // One local confirmation pass only (no multi-attempt terminal
                                // loop). Midstream already had multiple chances.
                                #[cfg(target_os = "windows")]
                                {
                                    if candidate.pcm.len() >= LOCAL_CONFIRMATION_START_BYTES {
                                        match confirm_terminal_local_windows(
                                            inner,
                                            &candidate.pcm,
                                            &phrase,
                                            &mut local_confirmation_ms,
                                            embedded_session_id,
                                        )
                                        .await
                                        {
                                            Some((result, origin_bytes))
                                                if terminal_local_result_can_activate(&result) =>
                                            {
                                                phrase_signal = denzic_voice_activation_v1_core::PhraseSignal::LocalTranscript;
                                                let end_seconds =
                                                    refined_wake_end_seconds(
                                                        0.0,
                                                        &result,
                                                        phrase.chars().count(),
                                                    )
                                                        + origin_bytes as f32 / 32_000.0;
                                                Some(crate::wake_phrase::Match {
                                                    start_seconds: None,
                                                    end_seconds,
                                                    matched_keyword: None,
                                                })
                                            }
                                            Some((result, _))
                                                if enrolled_terminal_local_near_can_accept(
                                                    enrolled_owner_matched,
                                                    &result,
                                                    phrase.chars().count(),
                                                    0,
                                                ) =>
                                            {
                                                    phrase_signal = denzic_voice_activation_v1_core::PhraseSignal::LocalTranscript;
                                                    log::info!(
                                                        "[wake-phrase] terminal enrolled owner recovered start-aligned local near-match embedded_session_id={} prefix_units={} distance={} transcript_chars={}",
                                                        embedded_session_id,
                                                        result.phonetic_prefix_units,
                                                        result.phonetic_best_distance,
                                                        result.transcript_chars
                                                    );
                                                    Some(crate::wake_phrase::Match {
                                                        start_seconds: None,
                                                        end_seconds: LOCAL_ONLY_START_ENDPOINT_MAX_SECONDS,
                                                        matched_keyword: None,
                                                    })
                                            }
                                            Some((result, origin_bytes))
                                                if open_terminal_local_near_can_accept(
                                                    &verification,
                                                    &result,
                                                    phrase.chars().count(),
                                                    origin_bytes,
                                                ) =>
                                            {
                                                    phrase_signal = denzic_voice_activation_v1_core::PhraseSignal::LocalTranscript;
                                                    log::info!(
                                                        "[wake-phrase] terminal open near-phrase wake accepted (voiceprint floor) embedded_session_id={} prefix_units={} distance={} window_start={} transcript_chars={} owner_score={:.6}",
                                                        embedded_session_id,
                                                        result.phonetic_prefix_units,
                                                        result.phonetic_best_distance,
                                                        result.phonetic_best_window_start,
                                                        result.transcript_chars,
                                                        verification.as_ref().map(|result| result.score).unwrap_or_default()
                                                    );
                                                    Some(crate::wake_phrase::Match {
                                                        start_seconds: None,
                                                        end_seconds: LOCAL_ONLY_START_ENDPOINT_MAX_SECONDS,
                                                        matched_keyword: None,
                                                    })
                                            }
                                            Some((result, _)) => {
                                                    if result.phrase_relation
                                                        == crate::wake_phrase::LocalPhraseRelation::Absent
                                                        && result.phonetic_best_distance <= 2
                                                        && result.transcript_chars
                                                            > phrase.chars().count()
                                                    {
                                                        candidate.owner_near_phrase_confirmations =
                                                            candidate
                                                                .owner_near_phrase_confirmations
                                                                .saturating_add(1);
                                                        log::info!(
                                                            "[wake-phrase] terminal near-phrase evidence retained embedded_session_id={} confirmations={} distance={} window_start={} transcript_chars={}",
                                                            embedded_session_id,
                                                            candidate.owner_near_phrase_confirmations,
                                                            result.phonetic_best_distance,
                                                            result.phonetic_best_window_start,
                                                            result.transcript_chars
                                                        );
                                                    }
                                                    if overlap_degraded_owner_phrase_evidence(
                                                        &result,
                                                        phrase.chars().count(),
                                                        0,
                                                    ) {
                                                        candidate.local_owner_overlap_near_confirmations = candidate
                                                            .local_owner_overlap_near_confirmations
                                                            .saturating_add(1);
                                                        log::info!(
                                                            "[wake-phrase] terminal overlap-degraded owner phrase evidence embedded_session_id={} count={}/{} prefix_units={} distance={}",
                                                            embedded_session_id,
                                                            candidate.local_owner_overlap_near_confirmations,
                                                            OWNER_OVERLAP_NEAR_CONFIRMATIONS_REQUIRED,
                                                            result.phonetic_prefix_units,
                                                            result.phonetic_best_distance
                                                        );
                                                    }
                                                    None
                                            }
                                            None => None,
                                        }
                                    } else {
                                        None
                                    }
                                }
                                #[cfg(not(target_os = "windows"))]
                                {
                                    None
                                }
                            }
                        }
                    }
                }
                Err(err) => {
                    log::warn!(
                        "[wake-phrase] automatic candidate rejected because detection failed embedded_session_id={embedded_session_id}: {err}"
                    );
                    save_bounded_wake_diagnostic(
                        embedded_session_id,
                        "detector-failed",
                        &candidate.pcm,
                    );
                    let rejected_bytes = candidate.pcm.len();
                    candidate.record_outcome_fact(
                        self.pipeline_observation.as_ref(),
                        crate::observability::CandidateFactKind::Rejected,
                        "wake_phrase_detection_failed",
                        rejected_bytes,
                    );
                    candidate.record_outcome_fact(
                        self.pipeline_observation.as_ref(),
                        crate::observability::CandidateFactKind::Closed,
                        "wake_phrase_detection_failed",
                        0,
                    );
                    self.preserve_candidate_fact_ledger(&mut candidate);
                    reject_hidden_automatic_candidate(
                        inner,
                        "wake_phrase_detection_failed",
                        embedded_session_id,
                        HiddenCandidateTransportState::Ended,
                    );
                    return Ok(true);
                }
            };
            // Firmware VoiceActivation is a VAD transport window, not phrase
            // evidence. It must never be relabelled as KeywordModel merely
            // because the enrolled owner is speaking. Interference recovery
            // continues below through separated phrase + owner verification.
            if wake_match.is_none()
                && verification.as_ref().is_ok_and(|result| {
                    crate::speech_decision_kernel::repeated_owner_near_phrase_wake_can_activate(
                        result.enrolled_owner_matched(),
                        result.score,
                        candidate.owner_near_phrase_confirmations,
                    )
                })
            {
                phrase_signal = denzic_voice_activation_v1_core::PhraseSignal::LocalTranscript;
                wake_match = Some(crate::wake_phrase::Match {
                    start_seconds: None,
                    end_seconds: LOCAL_ONLY_START_ENDPOINT_MAX_SECONDS,
                    matched_keyword: None,
                });
                log::info!(
                    "[wake-phrase] terminal owner near-phrase recovery accepted embedded_session_id={} confirmations={} owner_score={:.6}",
                    embedded_session_id,
                    candidate.owner_near_phrase_confirmations,
                    verification.as_ref().map(|result| result.score).unwrap_or_default()
                );
            }
            if wake_match.is_none()
                && candidate.local_owner_overlap_near_confirmations > 0
                && verification.as_ref().is_ok_and(|result| {
                    crate::speech_decision_kernel::terminal_owner_overlap_wake_can_activate(
                        terminal_wake_source_owner_compatible(&verification),
                        result.score,
                        candidate.local_owner_overlap_near_confirmations,
                    )
                })
            {
                // The terminal full-buffer pass is the only usable window in
                // a suffix-cropped wake.  Preserve the owner's wake instead of
                // waiting for three windows that no longer contain the phrase.
                phrase_signal = denzic_voice_activation_v1_core::PhraseSignal::LocalTranscript;
                wake_match = Some(crate::wake_phrase::Match {
                    start_seconds: None,
                    end_seconds: LOCAL_ONLY_START_ENDPOINT_MAX_SECONDS,
                    matched_keyword: None,
                });
                log::info!(
                    "[wake-phrase] terminal owner overlap recovered with single strong confirmation embedded_session_id={} confirmations={} owner_score={:.6}",
                    embedded_session_id,
                    candidate.local_owner_overlap_near_confirmations,
                    verification.as_ref().map(|result| result.score).unwrap_or_default()
                );
            }
            if wake_match.is_none()
                && enrolled_owner_repeated_overlap_near_can_accept(
                    terminal_wake_source_owner_compatible(&verification),
                    candidate.local_owner_overlap_near_confirmations,
                )
            {
                // Mono overlap cannot reconstruct two masked characters. Only
                // three expanding, start-aligned confirmations plus compatible
                // persistent enrolled identity may recover the phrase.
                phrase_signal = denzic_voice_activation_v1_core::PhraseSignal::LocalTranscript;
                wake_match = Some(crate::wake_phrase::Match {
                    start_seconds: None,
                    end_seconds: LOCAL_ONLY_START_ENDPOINT_MAX_SECONDS,
                    matched_keyword: None,
                });
                log::info!(
                    "[wake-phrase] terminal enrolled owner recovered overlap-degraded phrase embedded_session_id={} confirmations={} end_s={:.3}",
                    embedded_session_id,
                    candidate.local_owner_overlap_near_confirmations,
                    LOCAL_ONLY_START_ENDPOINT_MAX_SECONDS
                );
            }
            let mut owner_verified_by_extraction = false;
            #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
            if wake_match.is_none()
                || (!enrolled_owner_matched
                    && phrase_signal != denzic_voice_activation_v1_core::PhraseSignal::None)
            {
                let source_owner_score = verification
                    .as_ref()
                    .map(|result| result.score)
                    .unwrap_or_default();
                let (interference_owner_rise, interference_baseline_score, baseline_samples) =
                    note_hidden_wake_interference_owner_score(
                        &mut candidate.wake_interference_baseline,
                        source_owner_score,
                        false,
                    );
                log::info!(
                    "[wake-phrase] interference owner baseline embedded_session_id={} source_owner_score={:.6} baseline_score={:.6} baseline_samples={} separated_verification_requested={}",
                    embedded_session_id,
                    source_owner_score,
                    interference_baseline_score,
                    baseline_samples,
                    interference_owner_rise
                );
                let source_owner_compatible =
                    terminal_wake_source_owner_compatible(&verification);
                let extraction_was_prefetched =
                    candidate.target_wake_extraction_task.is_some();
                maybe_start_terminal_owner_compatible_wake_extraction(
                    &mut candidate,
                    &phrase,
                    embedded_session_id,
                    &verification,
                    interference_owner_rise,
                    wake_match.is_some(),
                );
                let lazy_terminal_extraction_started = !extraction_was_prefetched
                    && candidate.target_wake_extraction_task.is_some();
                if let Some(evidence) = terminal_target_wake_evidence(
                    &mut candidate,
                    inner,
                    &phrase,
                    embedded_session_id,
                    lazy_terminal_extraction_started,
                    source_owner_compatible,
                )
                .await
                {
                    local_confirmation_ms = local_confirmation_ms
                        .saturating_add(evidence.local_confirmation_ms);
                    phrase_signal =
                        denzic_voice_activation_v1_core::PhraseSignal::LocalTranscript;
                    wake_match = Some(evidence.wake_match);
                    owner_verified_by_extraction = evidence.owner_verified_by_extraction;
                    log::info!(
                        "[target-speaker] terminal extracted enrolled-owner wake recovered embedded_session_id={} extraction_ms={} owner_score={:.6} residual_ratio={:.6}",
                        embedded_session_id,
                        evidence.extraction_ms,
                        evidence.owner_score,
                        evidence.residual_ratio
                    );
                }
            }
            let total_ms = candidate
                .kws_total_ms
                .saturating_add(local_confirmation_ms)
                .saturating_add(voiceprint_ms);
            // Installed sessions 210/212 proved terminal and live candidates
            // must share one fused phrase/owner policy.
            let effective_phrase_signal = wake_match
                .as_ref()
                .map(|_| phrase_signal)
                .unwrap_or(denzic_voice_activation_v1_core::PhraseSignal::None);
            let (owner_gate, arbitration) = arbitrate_candidate_wake(
                &mut candidate,
                effective_phrase_signal,
                &verification,
                owner_verified_by_extraction,
                true,
            );
            let gate_decision = arbitration.decision;
            let enrolled_owner_matched = arbitration.owner_access.enrolled_owner_verified();
            log::info!(
                "[wake-phrase] automatic streaming gate embedded_session_id={} terminal=true pcm_ms={} kws_fed_bytes={} kws_ms={} local_confirmation_ms={} voiceprint_ms={} total_compute_ms={} phrase_signal={:?} gate_decision={:?} owner_policy={} enrolled_owner_matched={} owner_recovered_by_local_phrase={}",
                embedded_session_id,
                candidate.pcm.len() / 32,
                candidate.kws_fed_bytes,
                candidate.kws_total_ms,
                local_confirmation_ms,
                voiceprint_ms,
                total_ms,
                effective_phrase_signal,
                gate_decision,
                arbitration.owner_access.policy_label(),
                enrolled_owner_matched,
                owner_gate.recovered_by_local_phrase
            );
            if wake_match.is_none()
                && gate_decision == denzic_voice_activation_v1_core::GateDecision::Accept
            {
                log::info!(
                    "[wake-phrase] terminal enrolled-owner keep-wake without local phrase embedded_session_id={embedded_session_id}"
                );
                wake_match = Some(crate::wake_phrase::Match {
                    start_seconds: None,
                    end_seconds: LOCAL_ONLY_START_ENDPOINT_MAX_SECONDS,
                    matched_keyword: None,
                });
            }
            let Some(wake_match) = wake_match else {
                log::info!(
                    "[wake-phrase] automatic candidate rejected embedded_session_id={} phrase={}",
                    embedded_session_id,
                    phrase
                );
                if let Some(sid) = take_early_capsule_session_id(&mut candidate) {
                    dismiss_early_wake_recording_capsule(inner, sid);
                }
                save_bounded_wake_diagnostic(
                    embedded_session_id,
                    "phrase-non-match",
                    &candidate.pcm,
                );
                let rejected_bytes = candidate.pcm.len();
                candidate.record_outcome_fact(
                    self.pipeline_observation.as_ref(),
                    crate::observability::CandidateFactKind::Rejected,
                    "wake_phrase_non_match",
                    rejected_bytes,
                );
                candidate.record_outcome_fact(
                    self.pipeline_observation.as_ref(),
                    crate::observability::CandidateFactKind::Closed,
                    "wake_phrase_non_match",
                    0,
                );
                self.preserve_candidate_fact_ledger(&mut candidate);
                reject_hidden_automatic_candidate(
                    inner,
                    "wake_phrase_non_match",
                    embedded_session_id,
                    HiddenCandidateTransportState::Ended,
                );
                return Ok(true);
            };
            if gate_decision != denzic_voice_activation_v1_core::GateDecision::Accept {
                let reason = match verification {
                    Ok(result) => {
                        log::info!(
                            "[speaker-verification] automatic candidate rejected embedded_session_id={} score={:.4}",
                            embedded_session_id,
                            result.score
                        );
                        "voiceprint_non_match"
                    }
                    Err(err) => {
                        log::warn!(
                            "[speaker-verification] automatic candidate rejected because verification failed embedded_session_id={embedded_session_id}: {err}"
                        );
                        "voiceprint_verification_failed"
                    }
                };
                if let Some(sid) = take_early_capsule_session_id(&mut candidate) {
                    dismiss_early_wake_recording_capsule(inner, sid);
                }
                save_bounded_wake_diagnostic(embedded_session_id, reason, &candidate.pcm);
                let rejected_bytes = candidate.pcm.len();
                candidate.record_outcome_fact(
                    self.pipeline_observation.as_ref(),
                    crate::observability::CandidateFactKind::Rejected,
                    reason,
                    rejected_bytes,
                );
                candidate.record_outcome_fact(
                    self.pipeline_observation.as_ref(),
                    crate::observability::CandidateFactKind::Closed,
                    reason,
                    0,
                );
                self.preserve_candidate_fact_ledger(&mut candidate);
                reject_hidden_automatic_candidate(inner, reason, embedded_session_id, HiddenCandidateTransportState::Ended);
                return Ok(true);
            }
            let result = match verification {
                Ok(result) => result,
                Err(_) => {
                    unreachable!("fail-closed speaker verification rejected the error above")
                }
            };
            log::info!(
                "[speaker-verification] automatic candidate decision embedded_session_id={} gate_allowed={} enrolled_owner_matched={} policy={} score={:.4} wake_phrase={} wake_end_s={:.3}",
                embedded_session_id,
                result.matched,
                enrolled_owner_matched,
                result.policy_label(),
                result.score,
                phrase,
                wake_match.end_seconds
            );
            local_speaker_seed = Some((
                candidate.pcm.clone(),
                wake_match.end_seconds,
                phrase.clone(),
                enrolled_owner_matched,
            ));
            save_bounded_wake_diagnostic(embedded_session_id, "accepted", &candidate.pcm);
            if phrase_signal == denzic_voice_activation_v1_core::PhraseSignal::KeywordModel {
                persist_verified_wake_phrase_calibration(phrase.clone()).await;
            }
            let post_wake_offset =
                post_wake_pcm_offset_bytes(wake_match.end_seconds, candidate.pcm.len());
            let post_wake_pcm_bytes = candidate.pcm.len().saturating_sub(post_wake_offset);
            // Capture the legal post-wake body before trimming the candidate to
            // its speaker anchor. The old terminal path retained only
            // `wake_pcm`, so the fresh continuation session started without
            // the already-received tail and could lose the first words.
            let terminal_wake_body = candidate.terminal_wake_body(post_wake_offset);
            let wake_anchor_offset =
                wake_speaker_anchor_pcm_offset_bytes(
                    wake_match.start_seconds,
                    wake_match.end_seconds,
                    candidate.pcm.len(),
                );
            let candidate_pcm_before = candidate.pcm.len();
            let pcm_base_before = candidate.pcm_base_offset;
            candidate.pcm.drain(..wake_anchor_offset);
            candidate.pcm_base_offset = candidate.pcm_base_offset.and_then(|base| {
                u64::try_from(wake_anchor_offset)
                    .ok()
                    .and_then(|offset| base.checked_add(offset))
            });
            log_wake_candidate_crop(
                "terminal",
                embedded_session_id,
                &candidate,
                &wake_match,
                candidate_pcm_before,
                pcm_base_before,
                post_wake_offset,
                wake_anchor_offset,
            );
            // Terminal accept often happens after the device already auto-stopped.
            // Opening a host dictation session with <1s post-wake scrap produces
            // empty ASR + Error capsule (owner: completely unusable).
            const MIN_POST_WAKE_DICTATION_PCM_BYTES: usize = 16_000 * 2; // 1.0 s
            // Terminal STOP means the VA window is over. Attaching dictation to
            // the dying clip produced 2152: 11 wake chars, then empty insert.
            // Always open a fresh host recording for the body.
            {
                log::info!(
                    "[wake-phrase] terminal accept requires body continuation embedded_session_id={} post_wake_pcm_ms={} min_ms={}",
                    embedded_session_id,
                    post_wake_pcm_bytes / 32,
                    MIN_POST_WAKE_DICTATION_PCM_BYTES / 32
                );
                if let Some(sid) = take_early_capsule_session_id(&mut candidate) {
                    dismiss_early_wake_recording_capsule(inner, sid);
                }
                if !stage_terminal_wake_continuation_with_body_at(
                    inner,
                    local_speaker_seed
                        .as_ref()
                        .expect("accepted terminal wake has a speaker seed")
                        .0
                        .clone(),
                    wake_match.end_seconds,
                    phrase.clone(),
                    enrolled_owner_matched,
                    terminal_wake_body,
                    Instant::now(),
                ) {
                    log::warn!(
                        "[wake-phrase] terminal continuation already active; rejecting duplicate embedded_session_id={embedded_session_id}"
                    );
                    let rejected_bytes = candidate.pcm.len();
                    candidate.record_outcome_fact(
                        self.pipeline_observation.as_ref(),
                        crate::observability::CandidateFactKind::Rejected,
                        "wake_phrase_continuation_already_active",
                        rejected_bytes,
                    );
                    candidate.record_outcome_fact(
                        self.pipeline_observation.as_ref(),
                        crate::observability::CandidateFactKind::Closed,
                        "wake_phrase_continuation_already_active",
                        0,
                    );
                    self.preserve_candidate_fact_ledger(&mut candidate);
                    reject_hidden_automatic_candidate(
                        inner,
                        "wake_phrase_continuation_already_active",
                        embedded_session_id,
                        HiddenCandidateTransportState::Ended,
                    );
                    return Ok(true);
                }
                save_bounded_wake_diagnostic(
                    embedded_session_id,
                    "accepted-terminal-continuation",
                    &candidate.pcm,
                );
                let continuation_requested = match request_embedded_ble_recording_start_from_host(
                    inner,
                    "terminal_wake_body_continuation",
                )
                .await
                {
                    Ok(session_id) => {
                        schedule_terminal_wake_continuation_expiry(
                            inner,
                            embedded_session_id,
                            session_id,
                        );
                        log::info!(
                            "[wake-phrase] terminal continuation recording requested embedded_session_id={embedded_session_id} coordinator_session_id={session_id}"
                        );
                        true
                    }
                    Err(err) => {
                        discard_terminal_wake_continuation(inner);
                        let _ = inner
                            .recording_lifecycle
                            .lock()
                            .close_candidate(embedded_session_id);
                        log::warn!(
                            "[wake-phrase] terminal continuation recording failed embedded_session_id={embedded_session_id}: {err}"
                        );
                        false
                    }
                };
                if !continuation_requested {
                    let rejected_bytes = candidate.pcm.len();
                    candidate.record_outcome_fact(
                        self.pipeline_observation.as_ref(),
                        crate::observability::CandidateFactKind::Rejected,
                        "terminal_continuation_request_failed",
                        rejected_bytes,
                    );
                }
                candidate.record_outcome_fact(
                    self.pipeline_observation.as_ref(),
                    crate::observability::CandidateFactKind::Closed,
                    if continuation_requested {
                        "terminal_continuation_staged"
                    } else {
                        "terminal_continuation_request_failed"
                    },
                    0,
                );
                self.preserve_candidate_fact_ledger(&mut candidate);
                return Ok(true);
            }
        } else {
            log::info!(
                "[speaker-verification] physical recording bypass embedded_session_id={embedded_session_id}"
            );
        }

        let mut session = match begin_embedded_audio_dictation_session(inner).await {
            Ok(session) => session,
            Err(err) => {
                candidate.record_outcome_fact(
                    self.pipeline_observation.as_ref(),
                    crate::observability::CandidateFactKind::Rejected,
                    "coordinator_session_begin_failed",
                    candidate.pcm.len(),
                );
                candidate.record_outcome_fact(
                    self.pipeline_observation.as_ref(),
                    crate::observability::CandidateFactKind::Closed,
                    "coordinator_session_begin_failed",
                    0,
                );
                self.preserve_candidate_fact_ledger(&mut candidate);
                return Err(err);
            }
        };
        session.attach_pipeline_observation(self.pipeline_observation.clone());
        if let Some((wake_pcm, wake_end_seconds, wake_phrase, enrolled_owner_matched)) =
            local_speaker_seed
        {
            session.start_local_speaker_tracking(
                wake_pcm,
                wake_end_seconds,
                wake_phrase,
                enrolled_owner_matched,
            );
        }
        if automatic
            && !inner
                .recording_lifecycle
                .lock()
                .promote_candidate_to_owner(embedded_session_id, session.session_id)
        {
            candidate.record_outcome_fact(
                self.pipeline_observation.as_ref(),
                crate::observability::CandidateFactKind::Rejected,
                "coordinator_lifecycle_promotion_rejected",
                candidate.pcm.len(),
            );
            candidate.record_outcome_fact(
                self.pipeline_observation.as_ref(),
                crate::observability::CandidateFactKind::Closed,
                "coordinator_lifecycle_promotion_rejected",
                0,
            );
            self.preserve_candidate_fact_ledger(&mut candidate);
            let _ = inner
                .recording_lifecycle
                .lock()
                .close_candidate(embedded_session_id);
            transition_pipeline_error_if_session_matches(inner, session.session_id);
            cancel_asr_for_session(inner, session.session_id);
            return Err(format!(
                "录音生命周期拒绝终端自动唤醒主人会话 embedded_session_id={embedded_session_id} coordinator_session_id={}",
                session.session_id
            ));
        }
        if !activate_embedded_audio_dictation_session(inner, session.session_id, 0.0) {
            candidate.record_outcome_fact(
                self.pipeline_observation.as_ref(),
                crate::observability::CandidateFactKind::Rejected,
                "coordinator_session_activation_cancelled",
                candidate.pcm.len(),
            );
            candidate.record_outcome_fact(
                self.pipeline_observation.as_ref(),
                crate::observability::CandidateFactKind::Closed,
                "coordinator_session_activation_cancelled",
                0,
            );
            self.preserve_candidate_fact_ledger(&mut candidate);
            let _ = inner
                .recording_lifecycle
                .lock()
                .close_owner(session.session_id);
            return Err("嵌入式音频听写会话已被取消".to_string());
        }
        if automatic {
            let capsule_audio_ms = (candidate.pcm.len() / 32) as u64;
            let phrase = inner.prefs.get().voice_wake_phrase;
            arm_accepted_automatic_wake_text_guard(
                inner,
                session.session_id,
                phrase,
                capsule_audio_ms,
                candidate.early_capsule_session_id.is_some(),
            );
        }
        // Promotion transfers the live dependency ledger itself. The session
        // and any finalization task must continue reading the same receipts
        // after the actor drops its candidate wrapper.
        session.source_admission_ledger = Arc::clone(&candidate.source_admission_ledger);
        crate::observability::begin_embedded_audio_session(session.session_id, embedded_session_id);
        let source_integrity_ledger = Arc::clone(&session.source_admission_ledger);
        register_embedded_source_integrity_ledger(inner, session.session_id, &source_integrity_ledger);
        self.session = Some(session);
        let session = self
            .session
            .as_mut()
            .ok_or_else(|| "嵌入式音频流式听写 session 尚未创建".to_string())?;
        crate::observability::record_embedded_audio_first_packet(session.session_id);
        let released_pcm_bytes = candidate.pcm.len();
        let destination_session_id = session.session_id.to_string();
        let destination_stream_id = Some(session.source_stream_id);
        let mut release_accepted_bytes = 0usize;
        let mut release_unaccepted_bytes = 0usize;
        let mut release_coordinates_unknown = false;
        let mut release_outcome_unknown = false;
        let mut release_offset = 0usize;
        while release_offset < candidate.pcm.len() {
            let release_end = (release_offset + EMBEDDED_AUDIO_FEED_CHUNK_BYTES)
                .min(candidate.pcm.len());
            let release_bytes = release_end - release_offset;
            let candidate_range = candidate_slice_range(
                candidate.pcm_base_offset,
                release_offset,
                release_bytes,
            );
            let source_runs = candidate.source_facts_for_range(candidate_range);
            let release_operation_id = candidate.next_operation_id();
            candidate.record_fact_with_operation_id(
                self.pipeline_observation.as_ref(),
                release_operation_id,
                crate::observability::CandidateFactKind::ReleaseAttempted,
                candidate_range,
                None,
                None,
                Some(destination_session_id.clone()),
                destination_stream_id,
                source_runs.clone(),
                release_bytes,
                None,
            );
            let destination_start = session.accepted_pcm_cursor.position();
            let accepted_bytes_before = session.streamed_pcm_bytes;
            let release_result = {
                let pcm = &candidate.pcm[release_offset..release_end];
                session.consume_streaming_pcm_from_segment(
                    inner,
                    pcm,
                    None,
                    Some(embedded_session_id),
                )
            };
            let release_source_interval = release_result.as_ref().ok().copied().flatten();
            let destination_range = destination_start
                .zip(session.accepted_pcm_cursor.position())
                .and_then(|(start, end)| {
                    (end >= start && end - start == release_bytes as u64).then_some(
                        crate::observability::PcmRange { start, end },
                    )
                });
            let accepted_bytes = session
                .streamed_pcm_bytes
                .saturating_sub(accepted_bytes_before);
            if accepted_bytes > 0 {
                let source_admission_operation_id =
                    session.next_source_admission_operation_id();
                record_candidate_release_source_dependencies(
                    &candidate,
                    candidate_range,
                    destination_range,
                    release_source_interval,
                    release_bytes,
                    accepted_bytes,
                    SourceAdmissionOperationOwner::Session {
                        session_id: session.session_id,
                    },
                    source_admission_operation_id,
                    &session.source_admission_ledger,
                );
            }
            match release_result {
                Ok(_) if accepted_bytes == release_bytes => {
                    release_accepted_bytes = release_accepted_bytes.saturating_add(accepted_bytes);
                    release_coordinates_unknown |= destination_range.is_none();
                    candidate.record_fact_with_operation_id(
                        self.pipeline_observation.as_ref(),
                        release_operation_id,
                        crate::observability::CandidateFactKind::ReleaseAccepted,
                        candidate_range,
                        None,
                        destination_range,
                        Some(destination_session_id.clone()),
                        destination_stream_id,
                        source_runs,
                        accepted_bytes,
                        if destination_range.is_some() {
                            None
                        } else {
                            Some("coordinator_acceptance_coordinates_unknown")
                        },
                    );
                }
                Ok(_) if accepted_bytes > 0 => {
                    release_accepted_bytes = release_accepted_bytes.saturating_add(accepted_bytes);
                    release_unaccepted_bytes = release_unaccepted_bytes
                        .saturating_add(release_bytes.saturating_sub(accepted_bytes));
                    release_outcome_unknown = true;
                    candidate.record_fact_with_operation_id(
                        self.pipeline_observation.as_ref(),
                        release_operation_id,
                        crate::observability::CandidateFactKind::ReleaseOutcomeUnknown,
                        None,
                        None,
                        None,
                        Some(destination_session_id.clone()),
                        destination_stream_id,
                        candidate.source_facts_for_range(None),
                        accepted_bytes,
                        Some("coordinator_partial_acceptance_range_unknown"),
                    );
                }
                Ok(_) => {
                    release_unaccepted_bytes =
                        release_unaccepted_bytes.saturating_add(release_bytes);
                    candidate.record_fact_with_operation_id(
                        self.pipeline_observation.as_ref(),
                        release_operation_id,
                        crate::observability::CandidateFactKind::ReleaseRejected,
                        candidate_range,
                        None,
                        destination_range,
                        Some(destination_session_id.clone()),
                        destination_stream_id,
                        source_runs,
                        release_bytes,
                        Some("coordinator_did_not_accept_release"),
                    );
                }
                Err(err) => {
                    candidate.record_fact_with_operation_id(
                        self.pipeline_observation.as_ref(),
                        release_operation_id,
                        crate::observability::CandidateFactKind::ReleaseRejected,
                        candidate_range,
                        None,
                        destination_range,
                        Some(destination_session_id.clone()),
                        destination_stream_id,
                        source_runs,
                        release_bytes,
                        Some("coordinator_rejected_release"),
                    );
                    session.candidate_id = Some(candidate.candidate_id);
                    session.candidate_fact_ledger = Some(std::mem::take(&mut candidate.fact_ledger));
                    return Err(err);
                }
            }
            release_offset = release_end;
        }
        let release_close_reason = if release_outcome_unknown {
            "release_partial_acceptance_range_unknown"
        } else if release_unaccepted_bytes > 0 {
            "release_not_fully_accepted"
        } else if release_coordinates_unknown {
            "released_to_coordinator_coordinates_unknown"
        } else {
            "released_to_coordinator"
        };
        candidate.record_outcome_fact(
            self.pipeline_observation.as_ref(),
            crate::observability::CandidateFactKind::Closed,
            release_close_reason,
            0,
        );
        session.candidate_id = Some(candidate.candidate_id);
        session.candidate_fact_ledger = Some(std::mem::take(&mut candidate.fact_ledger));
        log::info!(
            "[speaker-verification] released buffered candidate to ASR embedded_session_id={} pcm_bytes={} accepted_pcm_bytes={} unaccepted_pcm_bytes={}",
            embedded_session_id,
            released_pcm_bytes,
            release_accepted_bytes,
            release_unaccepted_bytes
        );
        Ok(false)
    }

    async fn promote_hidden_candidate_if_requested(
        &mut self,
        inner: &Arc<Inner>,
        embedded_session_id: u32,
    ) -> Result<bool, String> {
        // 先确认候选到达再消费 promotion：否则物理键按下后 promotion 在 candidate
        // 到达之前就被 take 掉（REQUESTED→NONE），candidate 随后到达却不再 promote，
        // 第一次按下进不了录音。candidate 没到时直接返回，promotion 留给下一个 chunk。
        let Some(mut candidate) = self.speaker_candidate.take() else {
            return Ok(false);
        };
        if candidate.kind != BufferedSpeakerCandidateKind::Verification {
            self.speaker_candidate = Some(candidate);
            return Ok(false);
        }
        if !take_hidden_automatic_candidate_promotion(inner, embedded_session_id) {
            self.speaker_candidate = Some(candidate);
            return Ok(false);
        }
        let candidate_observation = self.pipeline_observation.clone();
        if self.embedded_session_id != Some(embedded_session_id) {
            candidate.record_outcome_fact(
                candidate_observation.as_ref(),
                crate::observability::CandidateFactKind::Rejected,
                "candidate_session_mismatch",
                candidate.pcm.len(),
            );
            candidate.record_outcome_fact(
                candidate_observation.as_ref(),
                crate::observability::CandidateFactKind::Closed,
                "candidate_session_mismatch",
                0,
            );
            self.preserve_candidate_fact_ledger(&mut candidate);
            return Err(format!(
                "物理录音接管 session 不一致: current={:?}, incoming={embedded_session_id}",
                self.embedded_session_id
            ));
        }

        let discarded_pcm_bytes = candidate.pcm.len();
        candidate.record_outcome_fact(
            candidate_observation.as_ref(),
            crate::observability::CandidateFactKind::Discarded,
            "promoted_to_physical_recording",
            discarded_pcm_bytes,
        );
        candidate.record_outcome_fact(
            candidate_observation.as_ref(),
            crate::observability::CandidateFactKind::Closed,
            "promoted_to_physical_recording",
            0,
        );
        discard_pre_press_candidate_pcm(&mut candidate.pcm);
        let mut session = match begin_embedded_audio_dictation_session(inner).await {
            Ok(session) => session,
            Err(err) => {
                candidate.record_outcome_fact(
                    candidate_observation.as_ref(),
                    crate::observability::CandidateFactKind::Rejected,
                    "coordinator_session_begin_failed",
                    candidate.pcm.len(),
                );
                candidate.record_outcome_fact(
                    candidate_observation.as_ref(),
                    crate::observability::CandidateFactKind::Closed,
                    "coordinator_session_begin_failed",
                    0,
                );
                self.preserve_candidate_fact_ledger(&mut candidate);
                return Err(err);
            }
        };
        session.attach_pipeline_observation(self.pipeline_observation.clone());
        if !inner
            .recording_lifecycle
            .lock()
            .promote_candidate_to_owner(embedded_session_id, session.session_id)
        {
            candidate.record_outcome_fact(
                self.pipeline_observation.as_ref(),
                crate::observability::CandidateFactKind::Rejected,
                "coordinator_lifecycle_promotion_rejected",
                candidate.pcm.len(),
            );
            candidate.record_outcome_fact(
                self.pipeline_observation.as_ref(),
                crate::observability::CandidateFactKind::Closed,
                "coordinator_lifecycle_promotion_rejected",
                0,
            );
            self.preserve_candidate_fact_ledger(&mut candidate);
            let _ = inner
                .recording_lifecycle
                .lock()
                .close_candidate(embedded_session_id);
            transition_pipeline_error_if_session_matches(inner, session.session_id);
            cancel_asr_for_session(inner, session.session_id);
            return Err(format!(
                "录音生命周期拒绝物理接管 embedded_session_id={embedded_session_id} coordinator_session_id={}",
                session.session_id
            ));
        }
        if !activate_embedded_audio_dictation_session(inner, session.session_id, 0.0) {
            candidate.record_outcome_fact(
                candidate_observation.as_ref(),
                crate::observability::CandidateFactKind::Rejected,
                "coordinator_session_activation_cancelled",
                candidate.pcm.len(),
            );
            candidate.record_outcome_fact(
                candidate_observation.as_ref(),
                crate::observability::CandidateFactKind::Closed,
                "coordinator_session_activation_cancelled",
                0,
            );
            self.preserve_candidate_fact_ledger(&mut candidate);
            let _ = inner
                .recording_lifecycle
                .lock()
                .close_owner(session.session_id);
            return Err("物理录音接管会话已被取消".to_string());
        }
        crate::observability::begin_embedded_audio_session(session.session_id, embedded_session_id);
        session.source_admission_ledger = Arc::clone(&candidate.source_admission_ledger);
        session.candidate_id = Some(candidate.candidate_id);
        session.candidate_fact_ledger = Some(std::mem::take(&mut candidate.fact_ledger));
        let source_integrity_ledger = Arc::clone(&session.source_admission_ledger);
        register_embedded_source_integrity_ledger(inner, session.session_id, &source_integrity_ledger);
        self.session = Some(session);
        let session = self
            .session
            .as_mut()
            .ok_or_else(|| "物理录音接管 session 尚未创建".to_string())?;
        crate::observability::record_embedded_audio_first_packet(session.session_id);
        log::info!(
            "[speaker-verification] physical recording promoted hidden candidate at post-press boundary embedded_session_id={} discarded_pre_press_pcm_bytes={}",
            embedded_session_id,
            discarded_pcm_bytes
        );
        Ok(true)
    }

    async fn try_release_automatic_candidate(
        &mut self,
        inner: &Arc<Inner>,
        embedded_session_id: u32,
    ) -> Result<bool, String> {
        let candidate_observation = self.pipeline_observation.clone();
        // Firmware VoiceActivation means only that a VAD transport window is
        // open. Prefetch identity in parallel for latency, but never let that
        // identity result manufacture phrase evidence or activate by itself.
        let phrase = inner.prefs.get().voice_wake_phrase;
        if let Some(candidate) = self.speaker_candidate.as_mut() {
            if candidate.kind == BufferedSpeakerCandidateKind::Verification {
                maybe_prefetch_owner_verification(candidate, &phrase, embedded_session_id);
            }
        }
        #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
        {
            let completed_task = self.speaker_candidate.as_mut().and_then(|candidate| {
                candidate
                    .target_wake_extraction_task
                    .as_ref()
                    .is_some_and(|task| task.inner().is_finished())
                    .then(|| candidate.target_wake_extraction_task.take())
                    .flatten()
            });
            if let Some(task) = completed_task {
                let phrase = inner.prefs.get().voice_wake_phrase;
                let evidence = match task.await {
                    Ok(Ok(extracted)) => evaluate_target_wake_extraction(
                        inner,
                        extracted,
                        &phrase,
                        embedded_session_id,
                        false,
                    )
                    .await
                    .unwrap_or_else(|err| {
                        log::warn!(
                            "[target-speaker] live owner wake evidence failed embedded_session_id={embedded_session_id}: {err}"
                        );
                        None
                    }),
                    Ok(Err(err)) => {
                        log::warn!(
                            "[target-speaker] live owner wake extraction failed embedded_session_id={embedded_session_id}: {err}"
                        );
                        None
                    }
                    Err(err) => {
                        log::warn!(
                            "[target-speaker] live owner wake extraction task failed embedded_session_id={embedded_session_id}: {err}"
                        );
                        None
                    }
                };
                if let Some(evidence) = evidence {
                    let candidate = self
                        .speaker_candidate
                        .as_mut()
                        .ok_or_else(|| "自动唤醒候选已丢失".to_string())?;
                    if candidate.pending_phrase_match.is_none() {
                        log::info!(
                            "[target-speaker] extracted enrolled-owner wake accepted as pending phrase embedded_session_id={} extraction_ms={} owner_score={:.6} residual_ratio={:.6}",
                            embedded_session_id,
                            evidence.extraction_ms,
                            evidence.owner_score,
                            evidence.residual_ratio
                        );
                        candidate.pending_phrase_match = Some(PendingAutomaticPhraseMatch {
                            wake_match: evidence.wake_match,
                            phrase_signal:
                                denzic_voice_activation_v1_core::PhraseSignal::LocalTranscript,
                            local_confirmation_ms: evidence.local_confirmation_ms,
                            owner_verification_start_ms: OWNER_VERIFICATION_START_MS,
                            owner_verified_by_extraction: evidence.owner_verified_by_extraction,
                        });
                    }
                }
            }
        }
        let pending_phrase_match = {
            let Some(candidate) = self.speaker_candidate.as_mut() else {
                return Ok(false);
            };
            if candidate.kind != BufferedSpeakerCandidateKind::Verification {
                return Ok(false);
            }
            match candidate.pending_phrase_match.take() {
                Some(pending) if candidate.pcm.len() < pending.owner_verification_start_ms * 32 => {
                    candidate.pending_phrase_match = Some(pending);
                    return Ok(false);
                }
                pending => pending,
            }
        };
        let (
            wake_match,
            phrase_signal,
            local_confirmation_ms,
            kws_step_ms,
            owner_verified_by_extraction,
        ) = if let Some(pending) = pending_phrase_match
        {
            log::info!(
                    "[wake-phrase] pending phrase hit reached real owner window embedded_session_id={} pcm_ms={} owner_window_ms={}",
                    embedded_session_id,
                    self.speaker_candidate
                        .as_ref()
                        .map(|candidate| candidate.pcm.len() / 32)
                        .unwrap_or_default(),
                    OWNER_VERIFICATION_START_MS
                );
            (
                pending.wake_match,
                pending.phrase_signal,
                pending.local_confirmation_ms,
                0,
                pending.owner_verified_by_extraction,
            )
        } else {
            if !self
                .poll_wake_detector_init(inner, embedded_session_id)
                .await?
            {
                // Detector still loading — PCM is already buffered; try again next chunk.
                return Ok(false);
            }
            let (
                detector,
                new_pcm,
                incremental_pcm,
                prior_origin_bytes,
                rotation_start_bytes,
                requested_feed_start,
                incremental_feed_start,
            ) = {
                let candidate = self
                    .speaker_candidate
                    .as_mut()
                    .ok_or_else(|| "自动唤醒候选已丢失".to_string())?;
                if candidate.pcm.len().saturating_sub(candidate.kws_fed_bytes)
                    < STREAMING_KWS_FEED_BATCH_BYTES
                {
                    return Ok(false);
                }
                let Some(detector) = candidate.wake_detector.take() else {
                    let rejected_bytes = candidate.pcm.len();
                    candidate.record_outcome_fact(
                        candidate_observation.as_ref(),
                        crate::observability::CandidateFactKind::Rejected,
                        "wake_phrase_detector_unavailable",
                        rejected_bytes,
                    );
                    candidate.kind = BufferedSpeakerCandidateKind::Rejected;
                    candidate.pcm.clear();
                    reject_hidden_automatic_candidate(
                        inner,
                        "wake_phrase_detector_unavailable",
                        embedded_session_id,
                        HiddenCandidateTransportState::Streaming,
                    );
                    log::warn!(
                            "[wake-phrase] hidden candidate rejected because streaming detector is unavailable embedded_session_id={embedded_session_id}"
                        );
                    return Ok(false);
                };
                let rotation_start_bytes = rolling_kws_rotation_start(
                    candidate.pcm.len(),
                    candidate.kws_stream_origin_bytes,
                    candidate.kws_first_hit_at.is_some(),
                );
                if candidate.kws_fed_bytes == 0 && rotation_start_bytes.is_none() {
                    let preroll_skip = leading_quiet_prefix_bytes(&candidate.pcm);
                    if preroll_skip > 0 {
                        candidate.kws_stream_origin_bytes = preroll_skip;
                        log::info!(
                            "[wake-phrase] skipped quiet firmware pre-roll embedded_session_id={} pcm_ms={}",
                            embedded_session_id,
                            preroll_skip / 32
                        );
                    }
                }
                let feed_start = rotation_start_bytes.unwrap_or(candidate.kws_fed_bytes.max(
                    candidate.kws_stream_origin_bytes,
                ));
                let new_pcm = candidate.pcm[feed_start..].to_vec();
                let incremental_pcm = candidate.pcm[candidate.kws_fed_bytes..].to_vec();
                let incremental_feed_start = candidate.kws_fed_bytes;
                candidate.kws_fed_bytes = candidate.pcm.len();
                (
                    detector,
                    new_pcm,
                    incremental_pcm,
                    candidate.kws_stream_origin_bytes,
                    rotation_start_bytes,
                    feed_start,
                    incremental_feed_start,
                )
            };
            let phrase_for_rotation = phrase.clone();
            let wake_task = tauri::async_runtime::spawn_blocking(move || {
                let started = Instant::now();
                let (mut detector, stream_origin_bytes, rotated, pcm_to_feed, actual_feed_start) =
                    if let Some(rotation_start_bytes) = rotation_start_bytes {
                        match crate::wake_phrase::StreamingDetector::new(&phrase_for_rotation) {
                            Ok(fresh) => (
                                fresh,
                                rotation_start_bytes,
                                true,
                                new_pcm,
                                requested_feed_start,
                            ),
                            Err(err) => {
                                log::warn!(
                                    "[wake-phrase] rolling detector refresh failed; preserving current stream: {err}"
                                );
                                (
                                    detector,
                                    prior_origin_bytes,
                                    false,
                                    incremental_pcm,
                                    incremental_feed_start,
                                )
                            }
                        }
                    } else {
                        (
                            detector,
                            prior_origin_bytes,
                            false,
                            new_pcm,
                            requested_feed_start,
                        )
                    };
                let actual_feed_bytes = pcm_to_feed.len();
                let result = detector.accept_pcm(&pcm_to_feed);
                (
                    detector,
                    result,
                    started.elapsed().as_millis() as u64,
                    stream_origin_bytes,
                    rotated,
                    actual_feed_start,
                    actual_feed_bytes,
                )
            })
            .await;
            let (
                detector,
                wake_match,
                kws_step_ms,
                stream_origin_bytes,
                rotated,
                actual_feed_start,
                actual_feed_bytes,
            ) = match wake_task {
                Ok(result) => result,
                Err(err) => {
                    if let Some(candidate) = self.speaker_candidate.as_mut() {
                        candidate.record_fact(
                            candidate_observation.as_ref(),
                            crate::observability::CandidateFactKind::KwsFeedUnknown,
                            candidate.current_candidate_range(),
                            None,
                            None,
                            None,
                            None,
                            candidate.source_facts_for_range(None),
                            0,
                            Some("streaming_kws_task_join_unknown"),
                        );
                        candidate.kws_fed_bytes = incremental_feed_start;
                        let rejected_bytes = candidate.pcm.len();
                        candidate.record_outcome_fact(
                            candidate_observation.as_ref(),
                            crate::observability::CandidateFactKind::Rejected,
                            "wake_phrase_detector_task_failed",
                            rejected_bytes,
                        );
                        candidate.kind = BufferedSpeakerCandidateKind::Rejected;
                        candidate.pcm.clear();
                    }
                    reject_hidden_automatic_candidate(
                        inner,
                        "wake_phrase_detector_task_failed",
                        embedded_session_id,
                        HiddenCandidateTransportState::Streaming,
                    );
                    log::warn!(
                            "[wake-phrase] streaming detector task failed embedded_session_id={embedded_session_id}: {err}"
                        );
                    return Ok(false);
                }
            };
            let wake_match = {
                let candidate = self
                    .speaker_candidate
                    .as_mut()
                    .ok_or_else(|| "自动唤醒候选已丢失".to_string())?;
                candidate.wake_detector = Some(detector);
                candidate.kws_stream_origin_bytes = stream_origin_bytes;
                candidate.kws_total_ms = candidate.kws_total_ms.saturating_add(kws_step_ms);
                let kws_feed_range = actual_feed_start
                    .checked_add(actual_feed_bytes)
                    .filter(|end| *end <= candidate.pcm.len())
                    .and_then(|_| {
                        candidate_slice_range(
                            candidate.pcm_base_offset,
                            actual_feed_start,
                            actual_feed_bytes,
                        )
                    });
                let kws_feed_bytes = candidate
                    .pcm
                    .len()
                    .checked_sub(actual_feed_start)
                    .and_then(|available| (actual_feed_bytes <= available).then_some(actual_feed_bytes))
                    .unwrap_or(0);
                candidate.record_fact(
                    candidate_observation.as_ref(),
                    crate::observability::CandidateFactKind::KwsFed,
                    candidate.current_candidate_range(),
                    kws_feed_range,
                    None,
                    None,
                    None,
                    candidate.source_facts_for_range(kws_feed_range),
                    kws_feed_bytes,
                    None,
                );
                if rotated {
                    log::info!(
                        "[wake-phrase] rolling detector refreshed embedded_session_id={} origin_pcm_ms={} overlap_ms={}",
                        embedded_session_id,
                        stream_origin_bytes / 32,
                        STREAMING_KWS_ROTATE_OVERLAP_MS
                    );
                    #[cfg(target_os = "windows")]
                    if should_advance_local_confirmation_window(
                        rotated,
                        candidate.kws_first_hit_at.is_some(),
                        candidate.local_confirmation_window_origin_bytes,
                        stream_origin_bytes,
                        candidate.local_confirmation_attempts,
                        candidate.local_confirmation_task.is_some(),
                    ) {
                        candidate.local_confirmation_window_origin_bytes = stream_origin_bytes;
                        candidate.local_confirmation_attempts = 0;
                        candidate.local_confirmation_last_snapshot_bytes = 0;
                        candidate.local_confirmation_prefix_retry.reset_window();
                        log::info!(
                            "[wake-phrase] local confirmation window advanced embedded_session_id={} origin_pcm_ms={} overlap_ms={}",
                            embedded_session_id,
                            stream_origin_bytes / 32,
                            STREAMING_KWS_ROTATE_OVERLAP_MS
                        );
                    }
                }
                match wake_match {
                    Ok(found) => offset_streaming_wake_match(found, stream_origin_bytes),
                    Err(err) => {
                        let rejected_bytes = candidate.pcm.len();
                        candidate.record_outcome_fact(
                            candidate_observation.as_ref(),
                            crate::observability::CandidateFactKind::Rejected,
                            "wake_phrase_detector_failed",
                            rejected_bytes,
                        );
                        candidate.kind = BufferedSpeakerCandidateKind::Rejected;
                        candidate.pcm.clear();
                        reject_hidden_automatic_candidate(
                            inner,
                            "wake_phrase_detector_failed",
                            embedded_session_id,
                            HiddenCandidateTransportState::Streaming,
                        );
                        log::warn!(
                                "[wake-phrase] streaming detector failed embedded_session_id={embedded_session_id}: {err}"
                            );
                        return Ok(false);
                    }
                }
            };
            // XiaoAi-style cascade (product: sensitive StreamingDetector::new):
            //   stage-1 KWS high recall → stage-2 local wake verifier (precision)
            // Decision:
            //   Present/PresentLater → Accept LocalTranscript
            //   explicit Absent ×N → reject (anti "开始啥的" half-phrase)
            //   secondary timeout / helper down → fail-open KeywordModel
            // Never bare-KWS Accept without a budgeted secondary attempt, and
            // never infinite-hold on stage-2 (that was the sensitivity regression).
            let mut phrase_signal = denzic_voice_activation_v1_core::PhraseSignal::None;
            let mut local_confirmation_ms = 0u64;
            let kws_hit = wake_match.clone();
            if kws_hit.is_some() {
                if let Some(candidate) = self.speaker_candidate.as_mut() {
                    candidate.kws_phrase_detected = true;
                    if candidate.kws_first_hit_at.is_none() {
                        candidate.kws_first_hit_at = Some(Instant::now());
                        candidate.kws_first_hit_pcm_ms = Some(candidate.pcm.len() / 32);
                        log::info!(
                            "[wake-phrase] stage1 KWS hit embedded_session_id={} pcm_ms={} secondary_budget_ms={}",
                            embedded_session_id,
                            candidate.pcm.len() / 32,
                            KWS_SECONDARY_CONFIRM_BUDGET_MS
                        );
                    }
                    maybe_prefetch_owner_verification(candidate, &phrase, embedded_session_id);
                }
            }
            let wake_match = {
                #[cfg(target_os = "windows")]
                {
                    let completed_task = {
                        let candidate = self
                            .speaker_candidate
                            .as_mut()
                            .ok_or_else(|| "自动唤醒候选已丢失".to_string())?;
                        if candidate.local_confirmation_task.is_none() {
                            let window_origin_bytes = candidate
                                .local_confirmation_window_origin_bytes
                                .min(candidate.pcm.len());
                            let window_pcm_bytes = candidate
                                .pcm
                                .len()
                                .saturating_sub(window_origin_bytes);
                            let defer_exploratory =
                                should_defer_exploratory_local_confirmation_for_fast_preroll(
                                    kws_hit.is_some(),
                                    candidate.local_confirmation_attempts,
                                    window_pcm_bytes,
                                    candidate.started_at.elapsed(),
                                );
                            let exploratory_allowed = !defer_exploratory
                                && exploratory_local_confirmation_allowed(
                                    kws_hit.is_some(),
                                    candidate.local_absent_count,
                                    window_origin_bytes,
                                    candidate.local_confirmation_attempts,
                                );
                            let ladder_snapshot = exploratory_allowed
                                .then(|| {
                                    local_confirmation_snapshot_for_window(
                                        candidate.pcm.len(),
                                        window_origin_bytes,
                                        candidate.local_confirmation_attempts,
                                    )
                                })
                                .flatten();
                            // Stage-2 ASAP after KWS (0.8s floor), not the 1.8s ladder.
                            let new_audio_since_last = window_pcm_bytes
                                .saturating_sub(candidate.local_confirmation_last_snapshot_bytes);
                            let kws_immediate = kws_hit.is_some()
                                && !candidate.kws_prompted_local_confirm
                                && candidate.pcm.len() >= KWS_IMMEDIATE_LOCAL_CONFIRM_MIN_BYTES
                                && new_audio_since_last > 0;
                            let kws_retry = kws_hit.is_some()
                                && candidate.kws_prompted_local_confirm
                                && new_audio_since_last >= KWS_LOCAL_CONFIRM_RETRY_BYTES
                                && candidate.kws_local_absent_count
                                    < KWS_SECONDARY_ABSENT_REJECT_COUNT;
                            let prefix_retry = candidate.local_confirmation_prefix_retry.should_start(
                                ladder_snapshot.is_some(),
                                candidate.local_confirmation_attempts,
                                new_audio_since_last,
                            );
                            if ladder_snapshot.is_some()
                                || kws_immediate
                                || kws_retry
                                || prefix_retry
                            {
                                if ladder_snapshot.is_some() {
                                    candidate.local_confirmation_attempts += 1;
                                }
                                candidate
                                    .local_confirmation_prefix_retry
                                    .note_started(prefix_retry);
                                if kws_immediate || kws_retry {
                                    candidate.kws_prompted_local_confirm = true;
                                }
                                let snapshot_pcm_ms = candidate.pcm.len() / 32;
                                let threshold_pcm_ms = ladder_snapshot
                                    .map(|bytes| bytes / 32)
                                    .unwrap_or(snapshot_pcm_ms);
                                candidate.local_confirmation_last_snapshot_bytes =
                                    window_pcm_bytes;
                                // A keyword hit keeps the established full-candidate
                                // confirmation. Exploratory local confirmation follows
                                // the same rolling origin as KWS so early ambient
                                // Absents cannot permanently veto a later wake phrase.
                                let task_origin_bytes = if kws_hit.is_some() {
                                    0
                                } else {
                                    window_origin_bytes
                                };
                                let confirmation_pcm =
                                    candidate.pcm[task_origin_bytes..].to_vec();
                                let confirmation_pcm_ms = confirmation_pcm.len() / 32;
                                candidate.local_confirmation_task_origin_bytes =
                                    task_origin_bytes;
                                candidate.local_confirmation_task_has_keyword_model_hit =
                                    kws_hit.is_some();
                                candidate.local_confirmation_task_started_at =
                                    Some(Instant::now());
                                candidate.local_confirmation_task = Some(
                                    spawn_local_wake_confirmation(
                                        inner,
                                        LocalWakeConfirmationDiagnosticContext {
                                            embedded_session_id,
                                            attempt: Some(candidate.local_confirmation_attempts),
                                            source_origin_bytes: task_origin_bytes,
                                            branch: if kws_hit.is_some() {
                                                "streaming-kws"
                                            } else {
                                                "streaming-exploratory"
                                            },
                                        },
                                        confirmation_pcm,
                                        phrase.clone(),
                                        kws_hit.is_none(),
                                    ),
                                );
                                log::info!(
                                        "[wake-phrase] stage2 local confirm started embedded_session_id={} attempt={} threshold_pcm_ms={} snapshot_pcm_ms={} window_origin_pcm_ms={} window_pcm_ms={} kws_hit={} kws_immediate={} kws_retry={} prefix_retry={}",
                                        embedded_session_id,
                                        candidate.local_confirmation_attempts,
                                        threshold_pcm_ms,
                                        snapshot_pcm_ms,
                                        task_origin_bytes / 32,
                                        confirmation_pcm_ms,
                                        kws_hit.is_some(),
                                        kws_immediate,
                                        kws_retry,
                                        prefix_retry
                                    );
                            }
                        }
                        if candidate
                            .local_confirmation_task
                            .as_ref()
                            .is_some_and(|task| task.inner().is_finished())
                        {
                            let task = candidate.local_confirmation_task.take();
                            candidate.local_confirmation_task_started_at = None;
                            task.map(|task| {
                                (
                                    task,
                                    candidate.local_confirmation_task_origin_bytes,
                                    candidate.local_confirmation_task_has_keyword_model_hit,
                                )
                            })
                        } else {
                            None
                        }
                    };
                    if let Some((task, task_origin_bytes, task_has_keyword_model_hit)) =
                        completed_task
                    {
                        let task_result = task.await;
                        let current_window_origin_bytes = self
                            .speaker_candidate
                            .as_ref()
                            .map(|candidate| candidate.local_confirmation_window_origin_bytes)
                            .unwrap_or_default();
                        let stale = local_confirmation_task_is_stale(
                            task_origin_bytes,
                            current_window_origin_bytes,
                            task_has_keyword_model_hit,
                        );
                        let stale_positive = matches!(
                            &task_result,
                            Ok(Ok(result))
                                if stale_local_confirmation_can_activate(stale, result, task_has_keyword_model_hit)
                        );
                        if stale && !stale_positive {
                            log::info!(
                                "[wake-phrase] stale local confirmation discarded embedded_session_id={} task_origin_pcm_ms={} current_origin_pcm_ms={}",
                                embedded_session_id,
                                task_origin_bytes / 32,
                                current_window_origin_bytes / 32
                            );
                            None
                        } else {
                            if stale_positive {
                                log::info!(
                                    "[wake-phrase] stale positive local confirmation preserved embedded_session_id={} task_origin_pcm_ms={} current_origin_pcm_ms={}",
                                    embedded_session_id,
                                    task_origin_bytes / 32,
                                    current_window_origin_bytes / 32
                                );
                            }
                            match task_result {
                                Ok(Ok(result)) => {
                                local_confirmation_ms = result.inference_ms;
                                log::info!(
                                        "[wake-phrase] stage2 local confirm finished embedded_session_id={} matched={} phrase_relation={:?} snapshot_pcm_ms={} transcript_chars={} phonetic_prefix_units={} phonetic_best_distance={} phonetic_best_window_start={} inference_ms={} window_origin_pcm_ms={} kws_hit={}",
                                        embedded_session_id,
                                        result.matched,
                                        result.phrase_relation,
                                        result.snapshot_pcm_ms,
                                        result.transcript_chars,
                                        result.phonetic_prefix_units,
                                        result.phonetic_best_distance,
                                        result.phonetic_best_window_start,
                                        result.inference_ms,
                                        task_origin_bytes / 32,
                                        task_has_keyword_model_hit
                                );
                                let fusion_matched = result.matched
                                    && local_confirmation_can_activate(
                                        task_has_keyword_model_hit,
                                        result.phrase_relation,
                                    );
                                if result.matched && !fusion_matched {
                                    if let Some(candidate) = self.speaker_candidate.as_mut() {
                                        candidate.local_kws_fusion_evidence = true;
                                    }
                                    log::info!(
                                        "[wake-phrase] local-only PresentLater held for stage1 embedded_session_id={} phrase_relation={:?}",
                                        embedded_session_id,
                                        result.phrase_relation
                                    );
                                }
                                if fusion_matched {
                                    let refined_end = kws_hit
                                        .as_ref()
                                        .map(|found| found.end_seconds)
                                        .unwrap_or(0.0);
                                    let refined_end = refined_wake_end_seconds(
                                        refined_end,
                                        &result,
                                        phrase.chars().count(),
                                    );
                                    let refined_end = if task_has_keyword_model_hit {
                                        refined_end
                                    } else {
                                        refined_end
                                            + task_origin_bytes as f32 / 32_000.0
                                    };
                                    phrase_signal = denzic_voice_activation_v1_core::PhraseSignal::LocalTranscript;
                                    if let Some(candidate) = self.speaker_candidate.as_mut() {
                                        show_early_wake_recording_capsule(inner, candidate);
                                    }
                                    Some(crate::wake_phrase::Match {
                                        start_seconds: kws_hit
                                            .as_ref()
                                            .and_then(|found| found.start_seconds),
                                        end_seconds: refined_end,
                                        matched_keyword: kws_hit
                                            .and_then(|found| found.matched_keyword),
                                    })
                                } else if !result.matched {
                                    let phrase_chars = phrase.chars().count();
                                    if result.phrase_relation
                                            == crate::wake_phrase::LocalPhraseRelation::Absent
                                        && result.phonetic_best_distance <= 2
                                        && result.transcript_chars > phrase_chars
                                    {
                                        if let Some(candidate) = self.speaker_candidate.as_mut() {
                                            candidate.owner_near_phrase_confirmations = candidate
                                                .owner_near_phrase_confirmations
                                                .saturating_add(1);
                                            log::info!(
                                                "[wake-phrase] near-phrase evidence retained embedded_session_id={} confirmations={} distance={} window_start={} transcript_chars={}",
                                                embedded_session_id,
                                                candidate.owner_near_phrase_confirmations,
                                                result.phonetic_best_distance,
                                                result.phonetic_best_window_start,
                                                result.transcript_chars
                                            );
                                        }
                                    }
                                    let (bounded_followup, current_window_origin_bytes) = self
                                        .speaker_candidate
                                        .as_ref()
                                        .map(|candidate| {
                                            (
                                                candidate
                                                    .local_confirmation_prefix_retry
                                                    .task_is_retry,
                                                candidate
                                                    .local_confirmation_window_origin_bytes,
                                            )
                                        })
                                        .unwrap_or_default();
                                    let live_owner_near = live_owner_near_wake_can_attempt(
                                        crate::speaker_verification::is_enrolled_for_phrase(
                                            &phrase,
                                        ),
                                        bounded_followup,
                                        &result,
                                        phrase_chars,
                                        task_origin_bytes,
                                        current_window_origin_bytes,
                                    );
                                    if live_owner_near {
                                        let wake_end_seconds = live_owner_near_wake_end_seconds(
                                            &result,
                                            phrase_chars,
                                            task_origin_bytes,
                                        );
                                        phrase_signal = denzic_voice_activation_v1_core::PhraseSignal::LocalTranscript;
                                        // This is deliberately weaker than a complete local
                                        // phrase match. Keep the candidate hidden until the
                                        // normal enrolled-owner arbitration below accepts it;
                                        // otherwise another speaker's near phrase could flash a
                                        // false Recording capsule before being rejected.
                                        log::info!(
                                            "[wake-phrase] bounded rolling owner near-match promoted to voiceprint gate embedded_session_id={} origin_pcm_ms={} prefix_units={} distance={} transcript_chars={} wake_end_s={:.3}",
                                            embedded_session_id,
                                            task_origin_bytes / 32,
                                            result.phonetic_prefix_units,
                                            result.phonetic_best_distance,
                                            result.transcript_chars,
                                            wake_end_seconds
                                        );
                                        Some(crate::wake_phrase::Match {
                                            start_seconds: Some(
                                                task_origin_bytes as f32 / 32_000.0,
                                            ),
                                            end_seconds: wake_end_seconds,
                                            matched_keyword: None,
                                        })
                                    } else {
                                    let absent = {
                                        let candidate = self
                                            .speaker_candidate
                                            .as_mut()
                                            .ok_or_else(|| "自动唤醒候选已丢失".to_string())?;
                                        note_local_confirmation_prefix(
                                            candidate,
                                            &result,
                                            phrase_chars,
                                            task_origin_bytes,
                                            embedded_session_id,
                                        );
                                        if candidate.local_confirmation_prefix_retry.pending {
                                            maybe_prefetch_owner_verification(
                                                candidate,
                                                &phrase,
                                                embedded_session_id,
                                            );
                                        }
                                        record_local_confirmation_absent(
                                            candidate,
                                            &result,
                                            phrase_chars,
                                            task_origin_bytes,
                                            task_has_keyword_model_hit,
                                            embedded_session_id,
                                        )
                                    };
                                    if !absent.prefix_retry && !task_has_keyword_model_hit {
                                        let pcm_ms = self
                                            .speaker_candidate
                                            .as_ref()
                                            .map(|c| c.pcm.len() / 32)
                                            .unwrap_or(0);
                                        log::info!(
                                            "[wake-phrase] local-only Absent recorded embedded_session_id={} count={} pcm_ms={} authoritative={}",
                                            embedded_session_id,
                                            absent.local_absent_count,
                                            pcm_ms,
                                            absent.authoritative_full_absent
                                        );
                                    } else if absent.counted_kws_absent
                                        && absent.kws_absent_count
                                            >= KWS_SECONDARY_ABSENT_REJECT_COUNT
                                    {
                                        log::info!(
                                            "[wake-phrase] stage2 Absent reject embedded_session_id={} count={} (anti half-phrase false wake)",
                                            embedded_session_id,
                                            absent.kws_absent_count
                                        );
                                    } else if absent.counted_kws_absent {
                                        log::info!(
                                            "[wake-phrase] stage2 Absent retry embedded_session_id={} count={}/{}",
                                            embedded_session_id,
                                            absent.kws_absent_count,
                                            KWS_SECONDARY_ABSENT_REJECT_COUNT
                                        );
                                    }
                                    None
                                    }
                                } else {
                                    None
                                }
                            }
                            Ok(Err(err)) => {
                                if crate::asr::local::wake_helper::is_busy_error(&err) {
                                    log::info!(
                                        "[wake-phrase] stage2 local confirm busy; retrying without queue embedded_session_id={} kws_hit={}",
                                        embedded_session_id,
                                        kws_hit.is_some()
                                    );
                                    None
                                } else {
                                    log::warn!(
                                        "[wake-phrase] stage2 local confirm unavailable embedded_session_id={embedded_session_id}: {err}"
                                    );
                                    let explicit_absent_count = self
                                        .speaker_candidate
                                        .as_ref()
                                        .and_then(|candidate| {
                                            kws_hit.as_ref().map(|wake_match| {
                                                keyword_fallback_absent_count(
                                                    candidate,
                                                    wake_match,
                                                )
                                            })
                                        })
                                        .unwrap_or(0);
                                    if secondary_fallback_can_accept_keyword(
                                        kws_hit.is_some(),
                                        explicit_absent_count,
                                    ) {
                                        let kws = kws_hit.expect("fallback requires keyword hit");
                                        phrase_signal =
                                            denzic_voice_activation_v1_core::PhraseSignal::KeywordModel;
                                        log::info!(
                                            "[wake-phrase] stage2 unavailable fail-open KeywordModel embedded_session_id={embedded_session_id}"
                                        );
                                        Some(kws)
                                    } else {
                                        log::info!(
                                            "[wake-phrase] stage2 unavailable held after explicit Absent embedded_session_id={} absent_count={}",
                                            embedded_session_id,
                                            explicit_absent_count
                                        );
                                        None
                                    }
                                }
                            }
                            Err(err) => {
                                log::warn!(
                                        "[wake-phrase] stage2 local confirm task failed embedded_session_id={embedded_session_id}: {err}"
                                    );
                                let explicit_absent_count = self
                                    .speaker_candidate
                                    .as_ref()
                                    .and_then(|candidate| {
                                        kws_hit.as_ref().map(|wake_match| {
                                            keyword_fallback_absent_count(candidate, wake_match)
                                        })
                                    })
                                    .unwrap_or(0);
                                if secondary_fallback_can_accept_keyword(
                                    kws_hit.is_some(),
                                    explicit_absent_count,
                                ) {
                                    let kws = kws_hit.expect("fallback requires keyword hit");
                                    phrase_signal =
                                        denzic_voice_activation_v1_core::PhraseSignal::KeywordModel;
                                    log::info!(
                                        "[wake-phrase] stage2 task-fail fail-open KeywordModel embedded_session_id={embedded_session_id}"
                                    );
                                    Some(kws)
                                } else {
                                    log::info!(
                                        "[wake-phrase] stage2 task failure held after explicit Absent embedded_session_id={} absent_count={}",
                                        embedded_session_id,
                                        explicit_absent_count
                                    );
                                    None
                                }
                            }
                        }
                        }
                    } else if let Some(kws) = kws_hit {
                        let kws_waited_ms = self
                            .speaker_candidate
                            .as_ref()
                            .and_then(|c| c.kws_first_hit_at)
                            .map(|t| t.elapsed().as_millis() as u64)
                            .unwrap_or(0);
                        let confirmation_attempt_ms = self
                            .speaker_candidate
                            .as_ref()
                            .and_then(|c| c.local_confirmation_task_started_at)
                            .map(|t| t.elapsed().as_millis() as u64)
                            .unwrap_or(0);
                        let waited_ms = effective_secondary_waited_ms(
                            kws_waited_ms,
                            confirmation_attempt_ms,
                        );
                        let explicit_absent_count = self
                            .speaker_candidate
                            .as_ref()
                            .map(|candidate| keyword_fallback_absent_count(candidate, &kws))
                            .unwrap_or(0);
                        match pending_secondary_decision(
                            true,
                            waited_ms,
                            explicit_absent_count,
                        ) {
                            PendingSecondaryDecision::HoldAfterExplicitAbsent => {
                                log::info!(
                                    "[wake-phrase] stage2 timeout held after explicit Absent embedded_session_id={} waited_ms={} kws_waited_ms={} secondary_attempt_ms={} budget_ms={} absent_count={}",
                                    embedded_session_id,
                                    waited_ms,
                                    kws_waited_ms,
                                    confirmation_attempt_ms,
                                    KWS_SECONDARY_CONFIRM_BUDGET_MS,
                                    explicit_absent_count
                                );
                                None
                            }
                            PendingSecondaryDecision::AwaitSecondary => {
                                // Wait for actual evidence even beyond the latency
                                // budget; pending work cannot authorize dictation.
                                None
                            }
                        }
                    } else {
                        // No stage-1 yet: local-only ladder may still Present.
                        None
                    }
                }
                #[cfg(not(target_os = "windows"))]
                {
                    if kws_hit.is_some() {
                        phrase_signal =
                            denzic_voice_activation_v1_core::PhraseSignal::KeywordModel;
                    }
                    kws_hit
                }
            };
            let Some(wake_match) = wake_match else {
                return Ok(false);
            };
            let phrase_enrolled =
                crate::speaker_verification::is_enrolled_for_phrase(&phrase);
            if self
                .speaker_candidate
                .as_ref()
                .is_some_and(|candidate| {
                    !owner_verification_window_ready(candidate.pcm.len(), phrase_enrolled)
                })
            {
                let candidate = self
                    .speaker_candidate
                    .as_mut()
                    .ok_or_else(|| "自动唤醒候选已丢失".to_string())?;
                log::info!(
                        "[wake-phrase] phrase hit pending real owner window embedded_session_id={} pcm_ms={} owner_window_ms={} phrase_signal={:?}",
                        embedded_session_id,
                        candidate.pcm.len() / 32,
                        OWNER_VERIFICATION_START_MS,
                        phrase_signal
                    );
                candidate.pending_phrase_match = Some(PendingAutomaticPhraseMatch {
                    wake_match,
                    phrase_signal,
                    local_confirmation_ms,
                    owner_verification_start_ms: OWNER_VERIFICATION_START_MS,
                    owner_verified_by_extraction: false,
                });
                return Ok(false);
            }
            (
                wake_match,
                phrase_signal,
                local_confirmation_ms,
                kws_step_ms,
                false,
            )
        };

        let candidate = self
            .speaker_candidate
            .as_mut()
            .ok_or_else(|| "自动唤醒候选已丢失".to_string())?;
        let pcm = candidate.pcm.clone();
        let pcm_ms = pcm.len() / 32;
        let kws_ms = candidate.kws_total_ms;
        let voiceprint_phrase = phrase.clone();
        let verification_task = match candidate.owner_verification_task.take() {
            Some(task) => task.await,
            None => {
                tauri::async_runtime::spawn_blocking(move || {
                    let started = Instant::now();
                    let result = crate::speaker_verification::verify(&pcm, &voiceprint_phrase);
                    (result, started.elapsed().as_millis() as u64)
                })
                .await
            }
        };
        let (verification, voiceprint_ms) = match verification_task {
            Ok(result) => result,
            Err(err) => (Err(format!("声纹验证任务失败: {err}")), 0),
        };
        let total_ms = kws_ms
            .saturating_add(local_confirmation_ms)
            .saturating_add(voiceprint_ms);
        let (owner_gate, arbitration) = arbitrate_candidate_wake(
            candidate,
            phrase_signal,
            &verification,
            owner_verified_by_extraction,
            false,
        );
        let gate_decision = arbitration.decision;
        let enrolled_owner_matched = arbitration.owner_access.enrolled_owner_verified();
        log::info!(
            "[wake-phrase] automatic streaming gate embedded_session_id={} terminal=false pcm_ms={} kws_fed_bytes={} kws_step_ms={} kws_ms={} local_confirmation_ms={} voiceprint_ms={} total_compute_ms={} phrase_signal={:?} gate_decision={:?} owner_policy={} enrolled_owner_matched={} owner_recovered_by_local_phrase={} owner_ambiguous_confirmations={} owner_best_ambiguous_score={:.6}",
            embedded_session_id,
            pcm_ms,
            candidate.kws_fed_bytes,
            kws_step_ms,
            kws_ms,
            local_confirmation_ms,
            voiceprint_ms,
            total_ms,
            phrase_signal,
            gate_decision,
            arbitration.owner_access.policy_label(),
            enrolled_owner_matched,
            owner_gate.recovered_by_local_phrase,
            candidate.owner_ambiguous_confirmations,
            candidate.owner_best_ambiguous_score
        );

        if gate_decision != denzic_voice_activation_v1_core::GateDecision::Accept {
            #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
            {
                maybe_start_phrase_owner_recovery(
                    candidate,
                    &phrase,
                    embedded_session_id,
                    phrase_signal,
                    enrolled_owner_matched,
                );
                if candidate.target_wake_extraction_task.is_some() {
                    candidate.pending_phrase_match = Some(PendingAutomaticPhraseMatch {
                        wake_match,
                        phrase_signal,
                        local_confirmation_ms,
                        owner_verification_start_ms: pcm_ms.saturating_add(1),
                        owner_verified_by_extraction: false,
                    });
                    log::info!(
                        "[target-speaker] phrase hit held for separated owner recovery embedded_session_id={} pcm_ms={} owner_score={:.6}",
                        embedded_session_id,
                        pcm_ms,
                        verification.as_ref().map(|result| result.score).unwrap_or_default()
                    );
                    return Ok(false);
                }
            }
            if let Some(retry_ms) =
                next_owner_verification_retry_after(pcm_ms, &verification)
            {
                candidate.pending_phrase_match = Some(PendingAutomaticPhraseMatch {
                    wake_match,
                    phrase_signal,
                    local_confirmation_ms,
                    owner_verification_start_ms: retry_ms,
                    owner_verified_by_extraction,
                });
                log::info!(
                    "[wake-phrase] phrase hit retained for owner retry embedded_session_id={} pcm_ms={} next_owner_window_ms={} verification={:?}",
                    embedded_session_id,
                    pcm_ms,
                    retry_ms,
                    verification.as_ref().map(|result| result.score)
                );
                return Ok(false);
            }
            save_bounded_wake_diagnostic(
                embedded_session_id,
                "voiceprint-non-match",
                &candidate.pcm,
            );
            if let Some(candidate) = self.speaker_candidate.as_mut() {
                let rejected_bytes = candidate.pcm.len();
                candidate.record_outcome_fact(
                    candidate_observation.as_ref(),
                    crate::observability::CandidateFactKind::Rejected,
                    "voiceprint_non_match",
                    rejected_bytes,
                );
                candidate.kind = BufferedSpeakerCandidateKind::Rejected;
                candidate.pcm.clear();
            }
            reject_hidden_automatic_candidate(
                inner,
                "voiceprint_non_match",
                embedded_session_id,
                HiddenCandidateTransportState::Streaming,
            );
            log::info!(
                "[wake-phrase] phrase matched but owner verification rejected embedded_session_id={} phrase={} result={:?}",
                embedded_session_id,
                phrase,
                verification.as_ref().map(|result| result.score)
            );
            return Ok(false);
        }
        if phrase_signal == denzic_voice_activation_v1_core::PhraseSignal::KeywordModel {
            persist_verified_wake_phrase_calibration(phrase.clone()).await;
        }

        let ensure_request_id = next_accepted_wake_capture_request_id();
        let previous_segment_id = embedded_session_id;
        let recording_control_task = tauri::async_runtime::spawn_blocking(move || {
            let started = Instant::now();
            let result = crate::embedded_ble::send_recording_control_ensure_wake_capture(
                ensure_request_id,
                previous_segment_id,
                Duration::from_secs(2),
            );
            (result, started.elapsed().as_millis() as u64)
        });
        let mut candidate = self
            .speaker_candidate
            .take()
            .ok_or_else(|| "自动唤醒候选已丢失".to_string())?;
        let local_speaker_seed = (
            candidate.pcm.clone(),
            wake_match.end_seconds,
            phrase.clone(),
            enrolled_owner_matched,
        );
        save_bounded_wake_diagnostic(embedded_session_id, "accepted", &candidate.pcm);
        // Keep only the bounded tail containing the accepted wake phrase. This
        // gives cloud diarization a target-speaker anchor without sending the
        // earlier ambient candidate; the wake-text guard keeps it out of UI/output.
        let post_wake_offset =
            post_wake_pcm_offset_bytes(wake_match.end_seconds, candidate.pcm.len());
        let post_wake_pcm_bytes = candidate.pcm.len().saturating_sub(post_wake_offset);
        let wake_anchor_offset =
            wake_speaker_anchor_pcm_offset_bytes(
                wake_match.start_seconds,
                wake_match.end_seconds,
                candidate.pcm.len(),
            );
        let candidate_pcm_before = candidate.pcm.len();
        let pcm_base_before = candidate.pcm_base_offset;
        candidate.pcm.drain(..wake_anchor_offset);
        candidate.pcm_base_offset = candidate.pcm_base_offset.and_then(|base| {
            u64::try_from(wake_anchor_offset)
                .ok()
                .and_then(|offset| base.checked_add(offset))
        });
        log_wake_candidate_crop(
            "live",
            embedded_session_id,
            &candidate,
            &wake_match,
            candidate_pcm_before,
            pcm_base_before,
            post_wake_offset,
            wake_anchor_offset,
        );
        let capsule_request_ms = candidate
            .early_capsule_request_ms
            .unwrap_or_else(|| candidate.started_at.elapsed().as_millis() as u64);
        let latency = denzic_observability_v1_core::assess_duration_ms(
            capsule_request_ms,
            denzic_observability_v1_core::PerformanceBudget {
                target_ms: 1_200,
                ceiling_ms: 1_500,
            },
        );
        let phrase_tail_to_capsule_ms =
            wake_phrase_tail_to_capsule_ms(wake_match.end_seconds, capsule_request_ms);
        let phrase_tail_latency = denzic_observability_v1_core::assess_duration_ms(
            phrase_tail_to_capsule_ms,
            denzic_observability_v1_core::PerformanceBudget {
                target_ms: 350,
                ceiling_ms: 500,
            },
        );
        let mut session = match begin_embedded_audio_dictation_session(inner).await {
            Ok(session) => session,
            Err(err) => {
                candidate.record_outcome_fact(
                    candidate_observation.as_ref(),
                    crate::observability::CandidateFactKind::Rejected,
                    "coordinator_session_begin_failed",
                    candidate.pcm.len(),
                );
                candidate.record_outcome_fact(
                    candidate_observation.as_ref(),
                    crate::observability::CandidateFactKind::Closed,
                    "coordinator_session_begin_failed",
                    0,
                );
                self.preserve_candidate_fact_ledger(&mut candidate);
                return Err(err);
            }
        };
        session.attach_pipeline_observation(self.pipeline_observation.clone());
        session.start_local_speaker_tracking(
            local_speaker_seed.0,
            local_speaker_seed.1,
            local_speaker_seed.2,
            local_speaker_seed.3,
        );
        if !inner
            .recording_lifecycle
            .lock()
            .promote_candidate_to_owner(embedded_session_id, session.session_id)
        {
            candidate.record_outcome_fact(
                candidate_observation.as_ref(),
                crate::observability::CandidateFactKind::Rejected,
                "coordinator_lifecycle_promotion_rejected",
                candidate.pcm.len(),
            );
            candidate.record_outcome_fact(
                candidate_observation.as_ref(),
                crate::observability::CandidateFactKind::Closed,
                "coordinator_lifecycle_promotion_rejected",
                0,
            );
            self.preserve_candidate_fact_ledger(&mut candidate);
            let _ = inner
                .recording_lifecycle
                .lock()
                .close_candidate(embedded_session_id);
            transition_pipeline_error_if_session_matches(inner, session.session_id);
            cancel_asr_for_session(inner, session.session_id);
            return Err(format!(
                "录音生命周期拒绝自动唤醒主人会话 embedded_session_id={embedded_session_id} coordinator_session_id={}",
                session.session_id
            ));
        }
        if !activate_embedded_audio_dictation_session(inner, session.session_id, 0.0) {
            candidate.record_outcome_fact(
                candidate_observation.as_ref(),
                crate::observability::CandidateFactKind::Rejected,
                "coordinator_session_activation_cancelled",
                candidate.pcm.len(),
            );
            candidate.record_outcome_fact(
                candidate_observation.as_ref(),
                crate::observability::CandidateFactKind::Closed,
                "coordinator_session_activation_cancelled",
                0,
            );
            self.preserve_candidate_fact_ledger(&mut candidate);
            let _ = inner
                .recording_lifecycle
                .lock()
                .close_owner(session.session_id);
            return Err("嵌入式音频听写会话已被取消".to_string());
        }
        let capsule_audio_ms = (candidate.pcm.len() / 32) as u64;
        arm_accepted_automatic_wake_text_guard(
            inner,
            session.session_id,
            phrase.clone(),
            capsule_audio_ms,
            candidate.early_capsule_session_id.is_some(),
        );
        // Automatic wake promotion keeps the candidate's live dependency
        // ledger; finalization must not fall back to the actor's bounded log.
        session.source_admission_ledger = Arc::clone(&candidate.source_admission_ledger);
        crate::observability::begin_embedded_audio_session(session.session_id, embedded_session_id);
        let source_integrity_ledger = Arc::clone(&session.source_admission_ledger);
        register_embedded_source_integrity_ledger(inner, session.session_id, &source_integrity_ledger);
        self.session = Some(session);
        self.accepted_wake_capture_ensure = Some(AcceptedWakeCaptureEnsure {
            request_id: ensure_request_id,
            previous_segment_id,
            requested_at: Instant::now(),
            deadline_at: Instant::now()
                + EMBEDDED_ACCEPTED_WAKE_CAPTURE_REPLACEMENT_TIMEOUT,
            replacement_wait_started_at: None,
            confirmed_segment_id: None,
        });
        // 2026-08-09 12:46:59 激活竞态：ENSURE 发出后旧唤醒段可能立即 complete
        // （仅含唤醒词）；竞态窗口内该段的 STOP 不得 finalize 本会话，正文在激活后
        // 的新设备段里。绑定由 Started/PcmChunk 处理分支完成。
        self.activation_segment_race_guard = Some((embedded_session_id, Instant::now()));
        let session = self
            .session
            .as_mut()
            .ok_or_else(|| "嵌入式音频流式听写 session 尚未创建".to_string())?;
        crate::observability::record_embedded_audio_first_packet(session.session_id);
        let destination_session_id = session.session_id.to_string();
        let destination_stream_id = Some(session.source_stream_id);
        let mut release_accepted_bytes = 0usize;
        let mut release_unaccepted_bytes = 0usize;
        let mut release_coordinates_unknown = false;
        let mut release_outcome_unknown = false;
        let mut release_offset = 0usize;
        while release_offset < candidate.pcm.len() {
            let release_end = (release_offset + EMBEDDED_AUDIO_FEED_CHUNK_BYTES)
                .min(candidate.pcm.len());
            let release_bytes = release_end - release_offset;
            let candidate_range = candidate_slice_range(
                candidate.pcm_base_offset,
                release_offset,
                release_bytes,
            );
            let source_runs = candidate.source_facts_for_range(candidate_range);
            let release_operation_id = candidate.next_operation_id();
            candidate.record_fact_with_operation_id(
                candidate_observation.as_ref(),
                release_operation_id,
                crate::observability::CandidateFactKind::ReleaseAttempted,
                candidate_range,
                None,
                None,
                Some(destination_session_id.clone()),
                destination_stream_id,
                source_runs.clone(),
                release_bytes,
                None,
            );
            let destination_start = session.accepted_pcm_cursor.position();
            let accepted_bytes_before = session.streamed_pcm_bytes;
            let release_result = {
                let pcm = &candidate.pcm[release_offset..release_end];
                session.consume_streaming_pcm_from_segment(
                    inner,
                    pcm,
                    None,
                    Some(embedded_session_id),
                )
            };
            let release_source_interval = release_result.as_ref().ok().copied().flatten();
            let destination_range = destination_start
                .zip(session.accepted_pcm_cursor.position())
                .and_then(|(start, end)| {
                    (end >= start && end - start == release_bytes as u64).then_some(
                        crate::observability::PcmRange { start, end },
                    )
                });
            let accepted_bytes = session
                .streamed_pcm_bytes
                .saturating_sub(accepted_bytes_before);
            if accepted_bytes > 0 {
                let source_admission_operation_id =
                    session.next_source_admission_operation_id();
                record_candidate_release_source_dependencies(
                    &candidate,
                    candidate_range,
                    destination_range,
                    release_source_interval,
                    release_bytes,
                    accepted_bytes,
                    SourceAdmissionOperationOwner::Session {
                        session_id: session.session_id,
                    },
                    source_admission_operation_id,
                    &session.source_admission_ledger,
                );
            }
            match release_result {
                Ok(_) if accepted_bytes == release_bytes => {
                    release_accepted_bytes = release_accepted_bytes.saturating_add(accepted_bytes);
                    release_coordinates_unknown |= destination_range.is_none();
                    candidate.record_fact_with_operation_id(
                        candidate_observation.as_ref(),
                        release_operation_id,
                        crate::observability::CandidateFactKind::ReleaseAccepted,
                        candidate_range,
                        None,
                        destination_range,
                        Some(destination_session_id.clone()),
                        destination_stream_id,
                        source_runs,
                        accepted_bytes,
                        if destination_range.is_some() {
                            None
                        } else {
                            Some("coordinator_acceptance_coordinates_unknown")
                        },
                    );
                }
                Ok(_) if accepted_bytes > 0 => {
                    release_accepted_bytes = release_accepted_bytes.saturating_add(accepted_bytes);
                    release_unaccepted_bytes = release_unaccepted_bytes
                        .saturating_add(release_bytes.saturating_sub(accepted_bytes));
                    release_outcome_unknown = true;
                    candidate.record_fact_with_operation_id(
                        candidate_observation.as_ref(),
                        release_operation_id,
                        crate::observability::CandidateFactKind::ReleaseOutcomeUnknown,
                        None,
                        None,
                        None,
                        Some(destination_session_id.clone()),
                        destination_stream_id,
                        candidate.source_facts_for_range(None),
                        accepted_bytes,
                        Some("coordinator_partial_acceptance_range_unknown"),
                    );
                }
                Ok(_) => {
                    release_unaccepted_bytes =
                        release_unaccepted_bytes.saturating_add(release_bytes);
                    candidate.record_fact_with_operation_id(
                        candidate_observation.as_ref(),
                        release_operation_id,
                        crate::observability::CandidateFactKind::ReleaseRejected,
                        candidate_range,
                        None,
                        destination_range,
                        Some(destination_session_id.clone()),
                        destination_stream_id,
                        source_runs,
                        release_bytes,
                        Some("coordinator_did_not_accept_release"),
                    );
                }
                Err(err) => {
                    candidate.record_fact_with_operation_id(
                        candidate_observation.as_ref(),
                        release_operation_id,
                        crate::observability::CandidateFactKind::ReleaseRejected,
                        candidate_range,
                        None,
                        destination_range,
                        Some(destination_session_id.clone()),
                        destination_stream_id,
                        source_runs,
                        release_bytes,
                        Some("coordinator_rejected_release"),
                    );
                    session.candidate_id = Some(candidate.candidate_id);
                    session.candidate_fact_ledger = Some(std::mem::take(&mut candidate.fact_ledger));
                    return Err(err);
                }
            }
            release_offset = release_end;
        }
        let release_close_reason = if release_outcome_unknown {
            "release_partial_acceptance_range_unknown"
        } else if release_unaccepted_bytes > 0 {
            "release_not_fully_accepted"
        } else if release_coordinates_unknown {
            "released_to_coordinator_coordinates_unknown"
        } else {
            "released_to_coordinator"
        };
        candidate.record_outcome_fact(
            candidate_observation.as_ref(),
            crate::observability::CandidateFactKind::Closed,
            release_close_reason,
            0,
        );
        session.candidate_id = Some(candidate.candidate_id);
        session.candidate_fact_ledger = Some(std::mem::take(&mut candidate.fact_ledger));
        // Awaiting actor-drained control here deadlocks PCM/VREC:SPEECH; observe it detached.
        let _recording_control_observer = tauri::async_runtime::spawn(async move {
            match recording_control_task.await {
                Ok((Ok(()), elapsed_ms)) => log::info!(
                    "[wake-phrase] accepted wake capture ensure sent request_id={ensure_request_id} previous_segment_id={previous_segment_id} elapsed_ms={elapsed_ms}",
                ),
                Ok((Err(err), elapsed_ms)) => log::warn!(
                    "[wake-phrase] accepted wake capture ensure failed request_id={ensure_request_id} previous_segment_id={previous_segment_id} elapsed_ms={elapsed_ms}: {err}"
                ),
                Err(err) => log::warn!(
                    "[wake-phrase] accepted wake capture ensure task failed request_id={ensure_request_id} previous_segment_id={previous_segment_id}: {err}"
                ),
            }
        });
        log::info!(
            "[wake-phrase] live automatic session activated and released embedded_session_id={} phrase={} phrase_signal={:?} wake_end_s={:.3} post_wake_pcm_bytes={} released_accepted_pcm_bytes={} released_unaccepted_pcm_bytes={} kws_ms={} local_confirmation_ms={} voiceprint_ms={} gate_total_ms={} recording_control=detached wake_to_capsule_request_ms={} latency_target_ms=1200 latency_target_pass={} latency_ceiling_ms=1500 latency_ceiling_pass={} phrase_tail_to_capsule_ms={} phrase_tail_target_ms=350 phrase_tail_target_pass={} phrase_tail_ceiling_ms=500 phrase_tail_ceiling_pass={}",
            embedded_session_id,
            phrase,
            phrase_signal,
            wake_match.end_seconds,
            post_wake_pcm_bytes,
            release_accepted_bytes,
            release_unaccepted_bytes,
            kws_ms,
            local_confirmation_ms,
            voiceprint_ms,
            total_ms,
            capsule_request_ms,
            latency.target_pass,
            latency.ceiling_pass,
            phrase_tail_to_capsule_ms,
            phrase_tail_latency.target_pass,
            phrase_tail_latency.ceiling_pass
        );
        Ok(true)
    }
}
