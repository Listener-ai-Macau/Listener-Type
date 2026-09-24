// EmbeddedStreamingDictation completion / cleanup path.
// Included into `coordinator::dictation` via `include!`.

impl EmbeddedStreamingDictation {
    fn abort_accepted_wake_capture_ensure(&mut self, reason: &'static str) {
        let Some(ensure) = self.accepted_wake_capture_ensure.take() else {
            return;
        };
        log::info!(
            "[wake-phrase] aborting accepted wake capture ensure request_id={} previous_segment_id={} reason={reason}",
            ensure.request_id,
            ensure.previous_segment_id
        );
        #[cfg(not(test))]
        tauri::async_runtime::spawn_blocking(move || {
            match crate::embedded_ble::send_recording_control_abort_wake_capture(
                ensure.request_id,
                Duration::from_secs(2),
            ) {
                Ok(()) => log::info!(
                    "[wake-phrase] accepted wake capture ensure abort sent request_id={} reason={reason}",
                    ensure.request_id
                ),
                Err(error) => log::warn!(
                    "[wake-phrase] accepted wake capture ensure abort failed request_id={} reason={reason}: {error}",
                    ensure.request_id
                ),
            }
        });
    }

    fn expire_accepted_wake_capture_if_due(&mut self, inner: &Arc<Inner>) -> bool {
        let Some(ensure) = self.accepted_wake_capture_ensure else {
            return false;
        };
        if ensure.confirmed_segment_id.is_some() {
            return false;
        }
        if Instant::now() < ensure.deadline_at {
            return false;
        }
        // r46f: an unconfirmed continuation handshake must not destroy a
        // session that is already producing recognized body text (the
        // confirmation marker can still be in flight while the wake-era
        // deadline expires — aborting there sealed 0 of 23 previewed chars).
        // With live body evidence the product session keeps running under the
        // ordinary endpoint contract; only the zombie case (no body ever)
        // stays a hard abort. The device-side ensure state is left to its own
        // latch timeouts instead of sending an abort that would cancel the
        // active recording.
        let live_session_id = self
            .session
            .as_ref()
            .map(|session| session.session_id);
        let body_live = live_session_id.is_some_and(|session_id| {
            automatic_wake_body_started(inner, session_id)
                || current_embedded_audio_partial_preview(inner)
                    .is_some_and(|text| !text.trim().is_empty())
        });
        if body_live {
            let request_id = ensure.request_id;
            let previous_segment_id = ensure.previous_segment_id;
            self.accepted_wake_capture_ensure.take();
            log::warn!(
                "[wake-phrase] accepted wake capture ensure timed out request_id={} previous_segment_id={} wait_ms={} replacement_wait_started={} action=keep_live_body_session",
                request_id,
                previous_segment_id,
                ensure.requested_at.elapsed().as_millis(),
                ensure.replacement_wait_started_at.is_some()
            );
            return false;
        }
        log::warn!(
            "[wake-phrase] accepted wake capture ensure timed out request_id={} previous_segment_id={} wait_ms={} replacement_wait_started={} action=abort_session",
            ensure.request_id,
            ensure.previous_segment_id,
            ensure.requested_at.elapsed().as_millis(),
            ensure.replacement_wait_started_at.is_some()
        );
        self.abort_accepted_wake_capture_ensure("continuation_timeout");
        cancel_session(inner);
        let _ = self.discard_active_session_after_user_cancel(inner);
        true
    }

    /// Close only lifecycle identity owned by this actor instance. A transport
    /// event without actor-local work must never mutate a newer product
    /// candidate/session.
    fn close_owned_product_lifecycle(&self, inner: &Arc<Inner>) {
        let mut lifecycle = inner.recording_lifecycle.lock();
        if let Some(session_id) = self.session.as_ref().map(|session| session.session_id) {
            let _ = lifecycle.close_owner(session_id);
        } else if self.speaker_candidate.is_some() {
            if let Some(embedded_session_id) = self.embedded_session_id {
                let _ = lifecycle.close_candidate(embedded_session_id);
            }
        }
    }

    /// Drop actor-local transport state after the product session was fully
    /// finalized by the no-body endpoint path.  There is deliberately no ASR
    /// cancel or error publication here: the provider result and Idle event
    /// have already been committed by `end_embedded_ble_session`.
    fn release_externally_finalized_session_if_needed(&mut self, inner: &Arc<Inner>) -> bool {
        let Some(session_id) = self.session.as_ref().map(|session| session.session_id) else {
            return false;
        };
        let product_closed = {
            let state = inner.state.lock();
            state.session_id == session_id && state.phase == SessionPhase::Idle
        };
        if !product_closed {
            return false;
        }
        clear_embedded_ble_awaiting_post_activation_segment(inner, session_id);
        self.close_owned_product_lifecycle(inner);
        self.reset_for_next_session();
        log::info!(
            "[embedded-ble] released actor transport state after logical no-body finalization session_id={session_id}"
        );
        true
    }

    async fn finish_completed_streaming_session(
        &mut self,
        inner: &Arc<Inner>,
        embedded_session_id: u32,
        expected_packet_count: u16,
    ) -> Result<(), String> {
        match self
            .finish_streaming_session(inner, embedded_session_id, expected_packet_count)
            .await
        {
            Ok(()) => {
                self.terminal_received = true;
                Ok(())
            }
            Err(err) if self.keep_notify_ready_after_completed_pipeline_error(inner, &err) => {
                Ok(())
            }
            Err(err) => Err(err),
        }
    }

    fn keep_notify_ready_after_completed_pipeline_error(
        &mut self,
        inner: &Arc<Inner>,
        err: &str,
    ) -> bool {
        if !self.keep_listening_after_pipeline_errors || self.session.is_some() {
            return false;
        }
        self.terminal_received = true;
        record_embedded_ble_session_actor_command(
            inner,
            EmbeddedBleSessionActorCommand::ActorRestart,
            None,
            format!("completed session pipeline error kept notify ready: {err}"),
        );
        log::warn!(
            "[embedded-ble] background session completed with dictation pipeline error; keeping notify open: {err}"
        );
        true
    }

    async fn finish_pending_stop_after_capture(
        &mut self,
        inner: &Arc<Inner>,
    ) -> Result<(), String> {
        let embedded_session_id = self
            .embedded_session_id
            .ok_or_else(|| "嵌入式 BLE 流式会话尚未收到开始包".to_string())?;
        let expected_packet_count = self
            .pending_stop_expected_packet_count
            .or_else(|| {
                self.collector
                    .inner()
                    .stats()
                    .expected_packet_count
                    .and_then(|count| u16::try_from(count).ok())
            })
            .ok_or_else(|| "嵌入式 BLE 流式会话尚未收到结束包".to_string())?;
        self.finish_completed_streaming_session(inner, embedded_session_id, expected_packet_count)
            .await
    }

    fn abort_streaming_session(
        &mut self,
        inner: &Arc<Inner>,
        embedded_session_id: u32,
        message: &str,
    ) {
        if self.embedded_session_id != Some(embedded_session_id) {
            return;
        }
        self.abort_active_session(inner, message);
    }

    fn abort_active_session(&mut self, inner: &Arc<Inner>, message: &str) {
        self.close_owned_product_lifecycle(inner);
        self.abort_accepted_wake_capture_ensure("stream_abort");
        self.activation_segment_race_guard = None;
        set_device_ai_processing_async(inner, false, "embedded_stream_abort");
        if matches!(
            self.speaker_candidate
                .as_ref()
                .map(|candidate| candidate.kind),
            Some(BufferedSpeakerCandidateKind::Enrollment)
        ) {
            crate::speaker_verification::fail_enrollment(message);
        }
        let event_session_id = self.session.as_ref().map(|session| session.session_id);
        if let Some(session_id) = event_session_id {
            clear_embedded_source_integrity_ledger(inner, session_id);
        }
        if let Some(mut session) = self.session.take() {
            self.preserve_session_candidate_fact_ledger(&mut session);
            crate::observability::record_embedded_audio_failure(session.session_id, message);
            cancel_asr_for_session(inner, session.session_id);
            restore_prepared_windows_ime_session(inner, session.session_id);
            publish_dictation_pipeline_error(inner, session.session_id, message.to_string());
        } else {
            if let Some(mut candidate) = self.speaker_candidate.take() {
                let rejected_bytes = candidate.pcm.len();
                candidate.record_outcome_fact(
                    self.pipeline_observation.as_ref(),
                    crate::observability::CandidateFactKind::Rejected,
                    "stream_aborted",
                    rejected_bytes,
                );
                candidate.record_outcome_fact(
                    self.pipeline_observation.as_ref(),
                    crate::observability::CandidateFactKind::Closed,
                    "stream_aborted",
                    0,
                );
                self.preserve_candidate_fact_ledger(&mut candidate);
            }
            let elapsed = inner.state.lock().started_at.elapsed().as_millis() as u64;
            emit_capsule(
                inner,
                CapsuleState::Error,
                0.0,
                elapsed,
                Some(message.to_string()),
                None,
            );
            // Session 90e1415f (2026-09-20 16:46): a stream abort that raced
            // session begin left the coordinator in Listening with an error
            // capsule that could never dismiss — schedule_capsule_idle only
            // fires on phase==Idle, and nothing else drove the state machine
            // down. Publish the pipeline error against the coordinator's
            // current session so the phase resets; when the state is already
            // Idle the event is ignored (InvalidPhase) and this is a no-op.
            let coordinator_session_id = inner.state.lock().session_id;
            if coordinator_session_id != uuid::Uuid::nil() {
                publish_dictation_pipeline_error(inner, coordinator_session_id, message.to_string());
            }
        }
        schedule_capsule_idle(inner, CAPSULE_STREAM_ERROR_HIDE_DELAY_MS, event_session_id);
        self.terminal_received = true;
    }

    /// User/capsule cancel while the continuous background notify must stay open.
    /// Dictation state is already cancelled by `cancel_session`; only clear stream-
    /// local buffers so the next START/PCM can form a new session without TYPE:BYE.
    fn discard_active_session_after_user_cancel(&mut self, inner: &Arc<Inner>) -> bool {
        let had_work = self.session.is_some()
            || self.speaker_candidate.is_some()
            || self.embedded_session_id.is_some();
        if !had_work {
            return false;
        }
        self.close_owned_product_lifecycle(inner);
        self.abort_accepted_wake_capture_ensure("user_cancel");
        set_device_ai_processing_async(inner, false, "embedded_stream_user_cancel");
        if matches!(
            self.speaker_candidate
                .as_ref()
                .map(|candidate| candidate.kind),
            Some(BufferedSpeakerCandidateKind::Enrollment)
        ) {
            crate::speaker_verification::fail_enrollment("用户取消录音");
        }
        if let Some(session_id) = self.session.as_ref().map(|session| session.session_id) {
            clear_embedded_source_integrity_ledger(inner, session_id);
        }
        if let Some(mut session) = self.session.take() {
            self.preserve_session_candidate_fact_ledger(&mut session);
            // A user can cancel after pause-early delivery has already pasted
            // text. Preserve the same provider timeline as a completed session
            // so a visible duplicate can be traced back to its ASR window.
            if record_embedded_audio_for_debug_enabled(inner) {
                if let Some(pcm) = session.archive_pcm.as_deref() {
                    let _ = archive_embedded_audio_if_enabled(inner, session.session_id, pcm);
                }
                if let Some(asr) = session.volcengine_asr.as_ref() {
                    if let Ok(path) = crate::persistence::asr_trace_path_for_session(
                        &session.session_id.to_string(),
                    ) {
                        let payload = serde_json::json!({
                            "sessionId": session.session_id.to_string(),
                            "cancelled": true,
                            "finalCoordinatorTextAvailable": false,
                            "finalizationSucceeded": false,
                            "streamedPcmBytes": session.streamed_pcm_bytes,
                            "normalizedPcmBytes": session.normalized_pcm_bytes,
                            "archivePcmBytes": session.archive_pcm.as_ref().map_or(0, Vec::len),
                            "audioDelivery": asr.diagnostic_audio_delivery(),
                            "trace": asr.take_diagnostic_trace(),
                        });
                        if let Ok(bytes) = serde_json::to_vec(&payload) {
                            if let Err(err) = std::fs::write(path, bytes) {
                                log::warn!("[coord] cancelled ASR diagnostic archive failed: {err}");
                            }
                        }
                    }
                }
            }
            cancel_asr_for_session(inner, session.session_id);
            restore_prepared_windows_ime_session(inner, session.session_id);
        }
        self.reset_for_next_session();
        log::info!(
            "[embedded-ble] discarded in-flight background stream session after user cancel; notify kept open"
        );
        true
    }

    /// Logical stream error on continuous background: drop local session state but
    /// keep the GATT notify subscription alive for the next attempt.
    fn discard_active_session_after_stream_error(&mut self, inner: &Arc<Inner>, message: &str) {
        self.close_owned_product_lifecycle(inner);
        set_device_ai_processing_async(inner, false, "embedded_stream_soft_error");
        if matches!(
            self.speaker_candidate
                .as_ref()
                .map(|candidate| candidate.kind),
            Some(BufferedSpeakerCandidateKind::Enrollment)
        ) {
            crate::speaker_verification::fail_enrollment(message);
        }
        if let Some(session_id) = self.session.as_ref().map(|session| session.session_id) {
            clear_embedded_source_integrity_ledger(inner, session_id);
        }
        if let Some(mut session) = self.session.take() {
            self.preserve_session_candidate_fact_ledger(&mut session);
            crate::observability::record_embedded_audio_failure(session.session_id, message);
            cancel_asr_for_session(inner, session.session_id);
            restore_prepared_windows_ime_session(inner, session.session_id);
            // Only surface error UI when a host session was live; candidate-only
            // wake rejections should not bounce the BLE link.
            publish_dictation_pipeline_error(inner, session.session_id, message.to_string());
        }
        if let Some(mut candidate) = self.speaker_candidate.take() {
            let rejected_bytes = candidate.pcm.len();
            candidate.record_outcome_fact(
                self.pipeline_observation.as_ref(),
                crate::observability::CandidateFactKind::Rejected,
                "stream_error",
                rejected_bytes,
            );
            candidate.record_outcome_fact(
                self.pipeline_observation.as_ref(),
                crate::observability::CandidateFactKind::Closed,
                "stream_error",
                0,
            );
            self.preserve_candidate_fact_ledger(&mut candidate);
        }
        self.reset_for_next_session();
        log::warn!(
            "[embedded-ble] discarded background stream session after error while keeping notify open: {message}"
        );
    }

    fn show_transcribing_after_stop(&self, inner: &Arc<Inner>) {
        if let Some(session) = self.session.as_ref() {
            let already_latched = embedded_audio_stop_feedback_latched(inner);
            latch_embedded_audio_stop_feedback(inner, session.session_id);
            if !already_latched {
                let _ = emit_embedded_audio_transcribing_if_active(
                    inner,
                    session.session_id,
                    current_embedded_audio_partial_preview(inner),
                );
            }
        }
    }

    fn submission_result(
        &self,
    ) -> Result<crate::embedded_audio::EmbeddedAudioSubmissionResult, String> {
        submission_result_from_stats(
            self.terminal_received,
            self.collector.inner().stats(),
            self.transcript.clone(),
        )
    }

    fn reset_for_next_session(&mut self) {
        self.abort_accepted_wake_capture_ensure("stream_reset");
        self.collector.reset();
        if let Some(mut session) = self.session.take() {
            // Keep candidate provenance even when the host session is reset
            // before a final snapshot can be emitted.
            self.preserve_session_candidate_fact_ledger(&mut session);
        }
        if let Some(mut candidate) = self.speaker_candidate.take() {
            // A reset without an explicit candidate terminal branch must not
            // invent an end reason. Preserve the still-open ledger so the
            // missing end remains observable as UNKNOWN.
            self.preserve_candidate_fact_ledger(&mut candidate);
        }
        self.embedded_session_id = None;
        self.transcript = None;
        self.pending_stop_expected_packet_count = None;
        self.pending_stop_force_after = None;
        self.activation_segment_race_guard = None;
        self.terminal_received = false;
        self.last_actor_pcm_consumed = false;
    }

    /// Continuous background: force-finish a STOP that never recovered missing
    /// packets, without tearing down the notify subscription.
    async fn force_finish_pending_stop_if_due(
        &mut self,
        inner: &Arc<Inner>,
    ) -> Result<bool, String> {
        let Some(deadline) = self.pending_stop_force_after else {
            return Ok(false);
        };
        if Instant::now() < deadline {
            return Ok(false);
        }
        let Some(expected) = self.pending_stop_expected_packet_count else {
            self.pending_stop_force_after = None;
            return Ok(false);
        };
        let Some(embedded_session_id) = self.embedded_session_id else {
            self.pending_stop_force_after = None;
            return Ok(false);
        };
        self.pending_stop_force_after = None;
        log::warn!(
            "[coord] continuous background forcing stop finish after drain wait embedded_session_id={embedded_session_id} expected={expected} received={} missing={}",
            self.collector.inner().stats().received_packet_count,
            self.collector.inner().stats().missing_packet_count
        );
        // Prefer the candidate/session finish path even with missing packets.
        match self
            .finish_completed_streaming_session(inner, embedded_session_id, expected)
            .await
        {
            Ok(()) => Ok(true),
            Err(err) => {
                log::warn!(
                    "[coord] continuous background forced stop finish failed; discarding session while keeping notify: {err}"
                );
                self.discard_active_session_after_stream_error(inner, &err);
                Ok(true)
            }
        }
    }

    fn into_submission_result(
        self,
    ) -> Result<crate::embedded_audio::EmbeddedAudioSubmissionResult, String> {
        submission_result_from_stats(
            self.terminal_received,
            self.collector.into_inner().stats(),
            self.transcript,
        )
    }

    fn into_cancelled_submission_result(
        self,
    ) -> crate::embedded_audio::EmbeddedAudioSubmissionResult {
        let collector = self.collector.into_inner();
        let mut stats = collector.stats();
        stats.end_reason = Some(crate::embedded_audio::SessionEndReason::Cancel);
        crate::embedded_audio::EmbeddedAudioSubmissionResult {
            reconstructed_pcm_bytes: stats.reconstructed_pcm_bytes,
            stats,
            transcript: None,
        }
    }
}
