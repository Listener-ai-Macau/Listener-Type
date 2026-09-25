// Embedded stream session attachment.
// Included into `coordinator::dictation` via `include!`.

impl EmbeddedStreamingDictation {
    fn background_listener() -> Self {
        Self {
            keep_listening_after_pipeline_errors: true,
            ..Self::default()
        }
    }

    async fn handle_notification(
        &mut self,
        inner: &Arc<Inner>,
        notification: &[u8],
    ) -> Result<bool, String> {
        self.handle_notification_with_capture_generation(inner, notification, None, None)
            .await
    }

    async fn handle_notification_with_capture_generation(
        &mut self,
        inner: &Arc<Inner>,
        notification: &[u8],
        capture_generation: Option<u64>,
        pipeline_observation: Option<
            Arc<crate::observability::EmbeddedAudioPipelineObservation>,
        >,
    ) -> Result<bool, String> {
        self.handle_notification_with_capture_generation_and_admission(
            inner,
            notification,
            capture_generation,
            pipeline_observation,
            None,
            None,
        )
        .await
    }

    async fn handle_notification_with_capture_generation_and_admission(
        &mut self,
        inner: &Arc<Inner>,
        notification: &[u8],
        capture_generation: Option<u64>,
        pipeline_observation: Option<
            Arc<crate::observability::EmbeddedAudioPipelineObservation>,
        >,
        capture_admission_fact: Option<crate::embedded_audio::SessionAdmissionFact>,
        capture_admission_receipt:
            Option<crate::embedded_audio::SessionAdmissionReceipt>,
    ) -> Result<bool, String> {
        let source_observation_is_explicit =
            capture_generation.is_some() || pipeline_observation.is_some();
        let observation = match (capture_generation, pipeline_observation) {
            (Some(capture_generation), Some(observation))
                if observation.capture_generation() == capture_generation => Some(observation),
            (Some(_), Some(_)) => {
                log::warn!(
                    "[obs-audio-pipeline] dropping mismatched pipeline observation for embedded notification"
                );
                None
            }
            (None, None) => self.pipeline_observation.clone(),
            (Some(_), None) => {
                log::warn!(
                    "[obs-audio-pipeline] embedded notification arrived without its pipeline observation"
                );
                None
            }
            (None, Some(observation)) => Some(observation),
        };
        if let Some(observation) = observation.as_ref() {
            observation.record_coordinator_channel_received(notification.len());
        }
        // SessionCollector resets its statistics as soon as it sees a different
        // START. A preceding STOP may still be waiting for missing tail packets;
        // that old product session must be settled while its own collector
        // state is intact, before the new physical segment is admitted.
        if self.keep_listening_after_pipeline_errors
            && self.pending_stop_expected_packet_count.is_some()
        {
            if let Ok(packet) = crate::embedded_audio::parse_packet(notification) {
                if packet.header.packet_type == crate::embedded_audio::PacketType::SessionStart {
                    let incoming_id = packet.header.session_id;
                    if self.embedded_session_id == Some(incoming_id) {
                        log::warn!(
                            "[coord] ignoring same-segment START after pending STOP embedded_session_id={incoming_id}"
                        );
                        return Ok(false);
                    }
                    if self.embedded_session_id.is_some()
                        && (self.session.is_some() || self.speaker_candidate.is_some())
                    {
                        let origin = crate::embedded_audio::SessionStartOrigin::from_wire(
                            packet.header.packet_pcm_bytes,
                        );
                        if !self.pending_stop_start_belongs_to_active_session(incoming_id, origin) {
                            let old_id = self.embedded_session_id.unwrap_or_default();
                            let stats = self.collector.inner().stats();
                            log::warn!(
                                "[coord] settling pending STOP before unowned START old_embedded_session_id={old_id} incoming_embedded_session_id={incoming_id} origin={origin:?} received={} missing={}",
                                stats.received_packet_count,
                                stats.missing_packet_count
                            );
                            if let Err(err) = self.finish_pending_stop_after_capture(inner).await {
                                log::warn!(
                                    "[coord] pending STOP could not finish before new START: {err}"
                                );
                                self.discard_active_session_after_stream_error(inner, &err);
                            } else {
                                if let Err(err) = self.submission_result() {
                                    log::warn!(
                                        "[coord] pending STOP submission incomplete before new START: {err}"
                                    );
                                }
                                self.reset_for_next_session();
                            }
                        }
                    }
                }
            }
        }
        let event = match self.collector.handle_notification(notification) {
            Ok(event) => event,
            Err(err) => {
                if let Some(observation) = observation.as_ref() {
                    observation.record_coordinator_parse_reject();
                }
                if capture_admission_fact
                    .as_ref()
                    .is_some_and(|fact| {
                        matches!(
                            fact.disposition,
                            crate::embedded_audio::SessionAdmissionDisposition::New
                                | crate::embedded_audio::SessionAdmissionDisposition::Replacement
                        )
                    })
                    || capture_admission_receipt.is_some()
                {
                    self.record_capture_admission_binding(
                        capture_generation,
                        capture_admission_fact,
                        capture_admission_receipt,
                        None,
                        None,
                        false,
                    );
                }
                return Err(format!("嵌入式音频流式包解析失败: {err}"));
            }
        };
        if let Some(observation) = observation.as_ref() {
            observation.record_coordinator_event(&event);
        }
        if let Some(observation) = observation.as_ref() {
            self.pipeline_observation = Some(Arc::clone(observation));
        }
        let embedded_session_id = match &event {
            crate::embedded_audio::StreamingSessionEvent::Started { session_id, .. }
            | crate::embedded_audio::StreamingSessionEvent::Stopped { session_id, .. }
            | crate::embedded_audio::StreamingSessionEvent::Cancelled { session_id, .. }
            | crate::embedded_audio::StreamingSessionEvent::Error { session_id, .. } => {
                Some(*session_id)
            }
            crate::embedded_audio::StreamingSessionEvent::PcmChunk(chunk) => {
                Some(chunk.session_id)
            }
            crate::embedded_audio::StreamingSessionEvent::Ignored(_) => None,
        };
        let had_pending_stop = self.pending_stop_expected_packet_count.is_some();
        let terminal_cleanup_event = matches!(
            &event,
            crate::embedded_audio::StreamingSessionEvent::Cancelled { .. }
                | crate::embedded_audio::StreamingSessionEvent::Error { .. }
        );
        let terminal_segment_id = match &event {
            crate::embedded_audio::StreamingSessionEvent::Stopped { session_id, .. }
            | crate::embedded_audio::StreamingSessionEvent::Cancelled { session_id, .. }
            | crate::embedded_audio::StreamingSessionEvent::Error { session_id, .. } => {
                Some(*session_id)
            }
            crate::embedded_audio::StreamingSessionEvent::PcmChunk(chunk)
                if had_pending_stop => Some(chunk.session_id),
            _ => None,
        };
        let actor_chunk_metadata = match &event {
            crate::embedded_audio::StreamingSessionEvent::PcmChunk(chunk) => chunk.metadata,
            _ => None,
        };
        let actor_admission_fact = self.collector.last_admission_fact();
        let source_admission_context = Some(CaptureAdmissionSourceContext {
            capture_generation,
            capture_fact: capture_admission_fact.clone(),
            capture_receipt: capture_admission_receipt.clone(),
            actor_fact: actor_admission_fact.clone(),
            actor_chunk_metadata,
        });
        let result = self
            .handle_ble_packet_actor_command_with_observation(
                inner,
                event,
                if source_observation_is_explicit {
                    observation.clone()
                } else {
                    None
                },
                source_observation_is_explicit,
                source_admission_context,
            )
            .await;
        if capture_admission_fact
            .as_ref()
            .is_some_and(|fact| {
                matches!(
                    fact.disposition,
                    crate::embedded_audio::SessionAdmissionDisposition::New
                        | crate::embedded_audio::SessionAdmissionDisposition::Replacement
                )
            })
            || capture_admission_receipt.is_some()
        {
            self.record_capture_admission_binding(
                capture_generation,
                capture_admission_fact,
                capture_admission_receipt,
                actor_admission_fact,
                actor_chunk_metadata,
                self.last_actor_pcm_consumed,
            );
        }
        let downstream_terminal = result.as_ref().is_ok_and(|completed| *completed)
            || (terminal_cleanup_event
                && self.session.is_none()
                && self.speaker_candidate.is_none());
        if terminal_segment_id.is_some() && downstream_terminal {
            if let Some(observation) = observation.as_ref() {
                observation.record_coordinator_terminal_drained(terminal_segment_id);
            }
        }
        if let Some(observation) = observation.as_ref() {
            observation.bind_sessions(
                self.session.as_ref().map(|session| session.session_id),
                embedded_session_id,
            );
            observation.snapshot("coordinator_applied", false);
        }
        result
    }

    fn pending_stop_start_belongs_to_active_session(
        &self,
        incoming_id: u32,
        origin: crate::embedded_audio::SessionStartOrigin,
    ) -> bool {
        if let Some(ensure) = self.accepted_wake_capture_ensure {
            return self.session.is_some()
                && incoming_id != ensure.previous_segment_id
                && Instant::now() <= ensure.deadline_at
                && (ensure.replacement_wait_started_at.is_some()
                    || self.activation_segment_race_guard.is_some())
                && embedded_ensure_start_origin_matches(origin, ensure.request_id);
        }
        self.session.is_some()
            && self.activation_segment_race_guard.is_some_and(|(previous_id, _)| {
                incoming_id != previous_id
                    && origin == crate::embedded_audio::SessionStartOrigin::VoiceActivation
            })
    }

    fn record_capture_admission_binding(
        &mut self,
        capture_generation: Option<u64>,
        capture_fact: Option<crate::embedded_audio::SessionAdmissionFact>,
        capture_receipt: Option<crate::embedded_audio::SessionAdmissionReceipt>,
        actor_fact: Option<crate::embedded_audio::SessionAdmissionFact>,
        actor_chunk_metadata: Option<crate::embedded_audio::StreamingPcmChunkMetadata>,
        actor_consumed: bool,
    ) {
        let status = capture_admission_binding_status(
            capture_generation,
            capture_fact.as_ref(),
            capture_receipt.as_ref(),
            actor_fact.as_ref(),
            actor_chunk_metadata,
            actor_consumed,
        );
        if self.capture_admission_bindings.len() >= CAPTURE_ADMISSION_BINDING_CAPACITY {
            self.capture_admission_bindings.pop_front();
            self.capture_admission_bindings_incomplete = true;
        }
        self.capture_admission_bindings.push_back(CaptureAdmissionBinding {
            capture_generation,
            capture_fact,
            capture_receipt,
            actor_fact,
            actor_chunk_metadata,
            actor_consumed,
            status,
        });
    }

    async fn handle_ble_packet_actor_command(
        &mut self,
        inner: &Arc<Inner>,
        event: crate::embedded_audio::StreamingSessionEvent,
    ) -> Result<bool, String> {
        self.handle_ble_packet_actor_command_with_observation(inner, event, None, false, None)
            .await
    }

    async fn handle_ble_packet_actor_command_with_observation(
        &mut self,
        inner: &Arc<Inner>,
        event: crate::embedded_audio::StreamingSessionEvent,
        source_observation: Option<
            Arc<crate::observability::EmbeddedAudioPipelineObservation>,
        >,
        source_observation_is_explicit: bool,
        source_admission_context: Option<CaptureAdmissionSourceContext>,
    ) -> Result<bool, String> {
        let event_detail = embedded_ble_session_event_detail(&event);
        let trace_timeline = embedded_ble_session_event_should_trace(&event);
        dispatch_embedded_ble_session_actor_command_with_trace(
            inner,
            EmbeddedBleSessionActorCommand::BlePacket,
            self.session.as_ref().map(|session| session.session_id),
            event_detail,
            trace_timeline,
            |_| (),
        );
        self.apply_ble_packet_actor_command(
            inner,
            event,
            source_observation,
            source_observation_is_explicit,
            source_admission_context,
        )
        .await
    }

    async fn begin_session_if_needed(
        &mut self,
        inner: &Arc<Inner>,
        embedded_session_id: u32,
    ) -> Result<(), String> {
        if self.session.is_some() {
            if self.embedded_session_id != Some(embedded_session_id) {
                return Err(format!(
                    "嵌入式音频流式 session 不一致: current={:?}, incoming={embedded_session_id}",
                    self.embedded_session_id
                ));
            }
            return Ok(());
        }

        // User-origin full session: do not auto-promote a later unrelated VA candidate.
        clear_device_key_dictation_takeover_pending(inner);
        self.embedded_session_id = Some(embedded_session_id);
        let mut session = begin_embedded_audio_dictation_session(inner).await?;
        session.attach_pipeline_observation(self.pipeline_observation.clone());
        let terminal_wake_continuation =
            take_terminal_wake_continuation(inner, session.session_id);
        if let Some(continuation) = terminal_wake_continuation.as_ref() {
            // The continuation owns the candidate's admission ledger. Reuse
            // that same ledger for the body bytes released below so the old
            // physical segment remains attributable after the actor promotes
            // the fresh coordinator session.
            session.source_admission_ledger =
                Arc::clone(&continuation.body.source_admission_ledger);
            session.start_local_speaker_tracking(
                continuation.wake_pcm.clone(),
                continuation.wake_end_seconds,
                continuation.wake_phrase.clone(),
                continuation.enrolled_owner_matched,
            );
            // Safety net: host-start arms the wake guard before PCM attaches, but
            // if begin_session raced and cleared it, re-arm so empty-body abandon
            // stays at 3.0s (body_started=false must not use snappy 1.0s).
            if !automatic_wake_session_active(inner, session.session_id) {
                arm_automatic_wake_text_guard(
                    inner,
                    session.session_id,
                    continuation.wake_phrase.clone(),
                    0,
                );
                // Capsule is already visible from the host-start Recording emit.
                acknowledge_automatic_wake_capsule_visible(inner, session.session_id);
            }
        }
        let lifecycle_admitted = if terminal_wake_continuation.is_some() {
            let mut lifecycle = inner.recording_lifecycle.lock();
            lifecycle
                .current_candidate_session_id()
                .is_some_and(|candidate_id| {
                    lifecycle.promote_candidate_to_owner(candidate_id, session.session_id)
                })
        } else {
            inner
                .recording_lifecycle
                .lock()
                .begin_manual_owner(embedded_session_id, session.session_id)
        };
        if !lifecycle_admitted {
            if let Some(candidate_id) = inner
                .recording_lifecycle
                .lock()
                .current_candidate_session_id()
            {
                let _ = inner
                    .recording_lifecycle
                    .lock()
                    .close_candidate(candidate_id);
            }
            transition_pipeline_error_if_session_matches(inner, session.session_id);
            cancel_asr_for_session(inner, session.session_id);
            return Err(format!(
                "录音生命周期拒绝主人会话 embedded_session_id={embedded_session_id} coordinator_session_id={}",
                session.session_id
            ));
        }
        if !activate_embedded_audio_dictation_session(inner, session.session_id, 0.0) {
            let _ = inner
                .recording_lifecycle
                .lock()
                .close_owner(session.session_id);
            if terminal_wake_continuation.is_some() {
                clear_automatic_wake_text_guard(inner);
            }
            return Err("嵌入式音频听写会话已被取消".to_string());
        }
        crate::observability::begin_embedded_audio_session(session.session_id, embedded_session_id);
        let source_integrity_ledger = Arc::clone(&session.source_admission_ledger);
        log::info!(
            "[coord] embedded audio streaming dictation started (embedded_session_id={embedded_session_id}, coordinator_session_id={}, asr={}, terminal_wake_continuation={})",
            session.session_id,
            session.active_asr,
            terminal_wake_continuation.is_some()
        );
        register_embedded_source_integrity_ledger(inner, session.session_id, &source_integrity_ledger);
        self.session = Some(session);
        if let Some(continuation) = terminal_wake_continuation {
            let session = self
                .session
                .as_mut()
                .ok_or_else(|| "嵌入式音频流式听写 session 尚未创建".to_string())?;
            session.release_terminal_wake_body(inner, continuation.body)?;
        }
        Ok(())
    }
}

fn capture_admission_binding_status(
    capture_generation: Option<u64>,
    capture_fact: Option<&crate::embedded_audio::SessionAdmissionFact>,
    capture_receipt: Option<&crate::embedded_audio::SessionAdmissionReceipt>,
    actor_fact: Option<&crate::embedded_audio::SessionAdmissionFact>,
    actor_chunk_metadata: Option<crate::embedded_audio::StreamingPcmChunkMetadata>,
    actor_consumed: bool,
) -> CaptureAdmissionBindingStatus {
    if capture_generation.is_none() {
        return CaptureAdmissionBindingStatus::CaptureGenerationMissing;
    }
    let Some(capture_fact) = capture_fact else {
        return CaptureAdmissionBindingStatus::CaptureFactMissing;
    };
    let Some(capture_receipt) = capture_receipt else {
        return CaptureAdmissionBindingStatus::CaptureReceiptMissing;
    };
    let witness = capture_receipt.witness();
    if witness.collector_instance_id != capture_fact.collector_instance_id
        || witness.reset_epoch != capture_fact.reset_epoch
        || witness.notification_id != capture_fact.notification_id
        || Some(witness.admission_id) != capture_fact.admission_id
        || Some(witness.physical_session_id) != capture_fact.physical_session_id
        || Some(witness.packet_sequence) != capture_fact.packet_sequence
    {
        return CaptureAdmissionBindingStatus::CaptureReceiptFactMismatch;
    }
    if capture_fact.metadata_incomplete
        || capture_fact.predecessor_unknown
        || witness.metadata_incomplete
    {
        return CaptureAdmissionBindingStatus::CaptureEvidenceIncomplete;
    }
    let Some(actor_fact) = actor_fact else {
        return CaptureAdmissionBindingStatus::ActorFactMissing;
    };
    if matches!(
        actor_fact.disposition,
        crate::embedded_audio::SessionAdmissionDisposition::Ignored(_)
    ) {
        return CaptureAdmissionBindingStatus::ActorIgnored;
    }
    if !matches!(
        actor_fact.disposition,
        crate::embedded_audio::SessionAdmissionDisposition::New
            | crate::embedded_audio::SessionAdmissionDisposition::Replacement
    ) || actor_fact.admission_id.is_none()
        || actor_fact.physical_session_id != capture_fact.physical_session_id
        || actor_fact.packet_sequence != capture_fact.packet_sequence
    {
        return CaptureAdmissionBindingStatus::ActorAdmissionMismatch;
    }
    let Some(actor_chunk_metadata) = actor_chunk_metadata else {
        return CaptureAdmissionBindingStatus::ActorMetadataMissing;
    };
    if actor_chunk_metadata.packet_sequence != capture_fact.packet_sequence.unwrap_or(u16::MAX) {
        return CaptureAdmissionBindingStatus::ActorMetadataMismatch;
    }
    if actor_fact.metadata_incomplete
        || actor_fact.predecessor_unknown
        || actor_chunk_metadata.metadata_incomplete
        || actor_chunk_metadata.collector_instance_id.is_none()
    {
        return CaptureAdmissionBindingStatus::ActorMetadataIncomplete;
    }
    if actor_consumed {
        CaptureAdmissionBindingStatus::MatchedConsumed
    } else {
        CaptureAdmissionBindingStatus::ActorNotConsumed
    }
}
