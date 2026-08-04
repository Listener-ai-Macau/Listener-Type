// EmbeddedStreamingDictation actor / PCM submit path.
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
        let event = self
            .collector
            .handle_notification(notification)
            .map_err(|err| format!("嵌入式音频流式包解析失败: {err}"))?;
        self.handle_ble_packet_actor_command(inner, event).await
    }

    async fn handle_ble_packet_actor_command(
        &mut self,
        inner: &Arc<Inner>,
        event: crate::embedded_audio::StreamingSessionEvent,
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
        self.apply_ble_packet_actor_command(inner, event).await
    }

    async fn apply_ble_packet_actor_command(
        &mut self,
        inner: &Arc<Inner>,
        event: crate::embedded_audio::StreamingSessionEvent,
    ) -> Result<bool, String> {
        match event {
            crate::embedded_audio::StreamingSessionEvent::Started { session_id, origin } => {
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
                        self.session = None;
                        self.speaker_candidate = None;
                        self.embedded_session_id = None;
                        self.pending_stop_expected_packet_count = None;
                        clear_hidden_automatic_candidate();
                    }
                    return Ok(false);
                }
                let chunk_session_id = chunk.session_id;
                if let Some(candidate) = self.speaker_candidate.as_mut() {
                    if candidate.kind == BufferedSpeakerCandidateKind::Rejected {
                        return Ok(false);
                    }
                    if candidate.pcm.len().saturating_add(chunk.pcm.len())
                        > MAX_BUFFERED_SPEAKER_CANDIDATE_BYTES
                    {
                        return Err("声纹候选录音超过安全缓冲上限".to_string());
                    }
                    candidate.pcm.extend_from_slice(&chunk.pcm);
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
                if embedded_streaming_chunk_is_asr_input(&chunk) {
                    self.begin_session_if_needed(inner, chunk.session_id)
                        .await?;
                    let proactive_stop_due = {
                        let session = self
                            .session
                            .as_mut()
                            .ok_or_else(|| "嵌入式音频流式听写 session 尚未创建".to_string())?;
                        crate::observability::record_embedded_audio_first_packet(session.session_id);
                        session.consume_streaming_pcm(
                            inner,
                            &chunk.pcm,
                            chunk.raw_input_level_percent,
                        )?;
                        // 改A: body started + sustained trailing silence + not yet dispatched.
                        session.proactive_stop_body_started
                            && session.proactive_stop_silence_ms
                                >= EMBEDDED_STREAMING_PROACTIVE_STOP_SILENCE_MS
                            && !session.proactive_stop_dispatched
                    };
                    if proactive_stop_due {
                        // Mirror stop_dictation: ask the firmware to cut the session
                        // short. The device then emits Stopped, which drives the normal
                        // finish_completed_streaming_session path — no bespoke finish.
                        let stop_sent = request_embedded_ble_recording_stop_from_host(
                            inner,
                            "proactive_trailing_silence",
                        )
                        .await
                        .unwrap_or(false);
                        if stop_sent {
                            if let Some(session) = self.session.as_mut() {
                                session.proactive_stop_dispatched = true;
                            }
                            log::info!(
                                "[coord] embedded audio proactive trailing-silence stop sent (session_id={}, silence_ms>={})",
                                chunk.session_id,
                                EMBEDDED_STREAMING_PROACTIVE_STOP_SILENCE_MS
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
                ..
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
                    if let Some(candidate) = self.speaker_candidate.take() {
                        clear_hidden_automatic_candidate();
                        if candidate.kind == BufferedSpeakerCandidateKind::Enrollment {
                            crate::speaker_verification::fail_enrollment("嵌入式音频会话已取消");
                            complete_voiceprint_enrollment_candidate(
                                "voiceprint_enrollment_cancelled",
                            );
                        } else {
                            reject_hidden_automatic_candidate(
                                "hidden_candidate_cancelled",
                                session_id,
                            );
                        }
                        self.reset_for_next_session();
                        return Ok(true);
                    }
                    // No host session and no candidate: keep notify (continuous path).
                    log::info!(
                        "[coord] embedded audio cancel without active stream work embedded_session_id={session_id}; keeping notify open"
                    );
                    self.reset_for_next_session();
                    return Ok(true);
                }
                if self.keep_listening_after_pipeline_errors {
                    self.discard_active_session_after_user_cancel(inner);
                    return Ok(true);
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
                    return Ok(true);
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
        clear_device_key_dictation_takeover_pending();
        self.embedded_session_id = Some(embedded_session_id);
        let session = begin_embedded_audio_dictation_session(inner).await?;
        if !activate_embedded_audio_dictation_session(inner, session.session_id, 0.0) {
            return Err("嵌入式音频听写会话已被取消".to_string());
        }
        crate::observability::begin_embedded_audio_session(session.session_id, embedded_session_id);
        log::info!(
            "[coord] embedded audio streaming dictation started (embedded_session_id={embedded_session_id}, coordinator_session_id={}, asr={})",
            session.session_id,
            session.active_asr
        );
        self.session = Some(session);
        Ok(())
    }

    async fn begin_candidate_or_session(
        &mut self,
        inner: &Arc<Inner>,
        embedded_session_id: u32,
        start_origin: crate::embedded_audio::SessionStartOrigin,
    ) -> Result<(), String> {
        if !recording_gate::try_admit_arc(
            inner,
            RecordIntent::BleBeginCandidateOrSession,
            &format!(
                "begin candidate/session embedded_session_id={embedded_session_id} origin={start_origin:?}"
            ),
        ) {
            return Ok(());
        }
        if self.session.is_some() || self.speaker_candidate.is_some() {
            if self.embedded_session_id != Some(embedded_session_id) {
                return Err(format!(
                    "嵌入式音频流式 session 不一致: current={:?}, incoming={embedded_session_id}",
                    self.embedded_session_id
                ));
            }
            return Ok(());
        }
        self.embedded_session_id = Some(embedded_session_id);
        let configured_phrase = inner.prefs.get().voice_wake_phrase;
        let mut kind = buffered_speaker_candidate_kind(
            start_origin,
            crate::speaker_verification::take_enrollment_arm(),
            crate::speaker_verification::is_enrolled_for_phrase(&configured_phrase),
        );
        if let Some(candidate_kind) = kind.take() {
            // Mark hidden ACTIVE before StreamingDetector::new (~1–2s). Device-key
            // Start during that window must promote (VREC:ACTIVATE) instead of
            // VREC:TOGGLE — toggle stops an in-flight VoiceActivation session, so
            // the first physical press looks like "recording failed" and only the
            // next press starts a clean User session.
            if candidate_kind == BufferedSpeakerCandidateKind::Verification {
                mark_hidden_automatic_candidate_active();
            }
            // Do NOT await StreamingDetector::new here — it costs ~0.5–1s wall time and
            // delayed the first PCM into the buffer until after the user finished 开始录音.
            // Spawn init in the background; PCM appends immediately; KWS starts when ready.
            let wake_detector_init =
                if candidate_kind == BufferedSpeakerCandidateKind::Verification {
                    let phrase = inner.prefs.get().voice_wake_phrase;
                    Some(tauri::async_runtime::spawn_blocking(move || {
                        // Primary wake uses StreamingDetector::new() (short-prefix variants
                        // + bootstrap threshold 0.08) for recall in noise / light slur.
                        crate::wake_phrase::StreamingDetector::new(&phrase)
                    }))
                } else {
                    None
                };
            if candidate_kind != BufferedSpeakerCandidateKind::Verification {
                clear_hidden_automatic_candidate();
            }
            log::info!(
                "[speaker-verification] buffering embedded candidate kind={candidate_kind:?} embedded_session_id={embedded_session_id} detector_deferred={}",
                wake_detector_init.is_some()
            );
            if candidate_kind == BufferedSpeakerCandidateKind::Verification {
                note_hidden_va_session(embedded_session_id);
            }
            self.speaker_candidate = Some(BufferedSpeakerCandidate {
                kind: candidate_kind,
                pcm: Vec::new(),
                wake_detector: None,
                wake_detector_init,
                pending_phrase_match: None,
                #[cfg(target_os = "windows")]
                local_confirmation_task: None,
                #[cfg(target_os = "windows")]
                local_confirmation_window_origin_bytes: 0,
                #[cfg(target_os = "windows")]
                local_confirmation_task_origin_bytes: 0,
                #[cfg(target_os = "windows")]
                local_confirmation_task_has_keyword_model_hit: false,
                local_confirmation_attempts: 0,
                local_confirmation_last_snapshot_bytes: 0,
                kws_prompted_local_confirm: false,
                kws_local_absent_count: 0,
                local_absent_count: 0,
                kws_first_hit_at: None,
                kws_first_hit_pcm_ms: None,
                early_capsule_session_id: None,
                kws_fed_bytes: 0,
                kws_stream_origin_bytes: 0,
                kws_total_ms: 0,
                started_at: Instant::now(),
            });
            return Ok(());
        }
        self.begin_session_if_needed(inner, embedded_session_id)
            .await
    }

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
            return Ok(());
        }
        self.show_transcribing_after_stop(inner);

        let mut session = self
            .session
            .take()
            .ok_or_else(|| "嵌入式音频流式听写 session 尚未创建".to_string())?;
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
        let end_result = end_embedded_ble_session(
            inner,
            user_initiated_stop,
            format!(
                "embedded_session_id={embedded_session_id} coordinator_session_id={} expected_packets={expected_packet_count}",
                coordinator_session_id
            ),
        )
        .await;
        if end_result.is_ok() {
            self.transcript = take_embedded_audio_final_result(inner, coordinator_session_id);
        }
        end_result
    }

    /// Complete deferred StreamingDetector::new if the init task has finished.
    /// Returns false while still loading (PCM continues to buffer).
    async fn poll_wake_detector_init(
        &mut self,
        inner: &Arc<Inner>,
        embedded_session_id: u32,
    ) -> Result<bool, String> {
        let candidate = match self.speaker_candidate.as_mut() {
            Some(candidate) => candidate,
            None => return Ok(false),
        };
        if candidate.wake_detector.is_some() {
            return Ok(true);
        }
        let Some(handle) = candidate.wake_detector_init.as_ref() else {
            return Ok(false);
        };
        if !handle.inner().is_finished() {
            return Ok(false);
        }
        let handle = candidate
            .wake_detector_init
            .take()
            .expect("wake_detector_init present");
        match handle.await {
            Ok(Ok(detector)) => {
                candidate.wake_detector = Some(detector);
                log::info!(
                    "[wake-phrase] deferred streaming detector ready embedded_session_id={embedded_session_id} buffered_pcm_ms={}",
                    candidate.pcm.len() / 32
                );
                Ok(true)
            }
            Ok(Err(err)) => {
                log::warn!(
                    "[wake-phrase] deferred streaming detector init failed embedded_session_id={embedded_session_id}: {err}"
                );
                candidate.kind = BufferedSpeakerCandidateKind::Rejected;
                if let Some(sid) = take_early_capsule_session_id(candidate) {
                    dismiss_early_wake_recording_capsule(inner, sid);
                }
                Ok(false)
            }
            Err(err) => {
                log::warn!(
                    "[wake-phrase] deferred streaming detector task failed embedded_session_id={embedded_session_id}: {err}"
                );
                candidate.kind = BufferedSpeakerCandidateKind::Rejected;
                if let Some(sid) = take_early_capsule_session_id(candidate) {
                    dismiss_early_wake_recording_capsule(inner, sid);
                }
                Ok(false)
            }
        }
    }

    async fn finish_buffered_speaker_candidate(
        &mut self,
        inner: &Arc<Inner>,
        embedded_session_id: u32,
    ) -> Result<bool, String> {
        let Some(mut candidate) = self.speaker_candidate.take() else {
            return Ok(false);
        };
        clear_hidden_automatic_candidate();
        if candidate.kind == BufferedSpeakerCandidateKind::Rejected {
            if let Some(sid) = take_early_capsule_session_id(&mut candidate) {
                dismiss_early_wake_recording_capsule(inner, sid);
            }
            reject_hidden_automatic_candidate(
                "automatic_candidate_rejected",
                embedded_session_id,
            );
            return Ok(true);
        }
        if candidate.kind == BufferedSpeakerCandidateKind::Enrollment {
            let pcm = candidate.pcm;
            let phrase = inner.prefs.get().voice_wake_phrase;
            let result = tauri::async_runtime::spawn_blocking(move || {
                crate::wake_phrase::calibrate(&pcm, &phrase)?;
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
            let Some(mut detector) = candidate.wake_detector.take() else {
                if let Some(sid) = take_early_capsule_session_id(&mut candidate) {
                    dismiss_early_wake_recording_capsule(inner, sid);
                }
                save_bounded_wake_diagnostic(
                    embedded_session_id,
                    "detector-unavailable",
                    &candidate.pcm,
                );
                reject_hidden_automatic_candidate(
                    "wake_phrase_detection_failed",
                    embedded_session_id,
                );
                return Ok(true);
            };
            let remaining_pcm = candidate.pcm[candidate.kws_fed_bytes..].to_vec();
            candidate.kws_fed_bytes = candidate.pcm.len();
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
                Ok(result) => result,
                Err(err) => {
                    log::warn!(
                        "[wake-phrase] terminal streaming detector task failed embedded_session_id={embedded_session_id}: {err}"
                    );
                    reject_hidden_automatic_candidate(
                        "wake_phrase_detection_failed",
                        embedded_session_id,
                    );
                    return Ok(true);
                }
            };
            candidate.kws_total_ms = candidate.kws_total_ms.saturating_add(final_kws_ms);
            let mut phrase_signal = denzic_voice_activation_v1_core::PhraseSignal::KeywordModel;
            let mut local_confirmation_ms = 0u64;
            let wake_match = match wake_match.map(|found| {
                offset_streaming_wake_match(found, stream_origin_bytes)
            }) {
                Ok(Some(found)) => {
                    #[cfg(target_os = "windows")]
                    {
                        let confirm = spawn_local_wake_confirmation(
                            inner,
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
                                    "[wake-phrase] terminal KWS local confirmation finished embedded_session_id={} matched={} phrase_relation={:?} snapshot_pcm_ms={} transcript_chars={} inference_ms={}",
                                    embedded_session_id,
                                    result.matched,
                                    result.phrase_relation,
                                    result.snapshot_pcm_ms,
                                    result.transcript_chars,
                                    result.inference_ms
                                );
                                if result.matched {
                                    phrase_signal =
                                        denzic_voice_activation_v1_core::PhraseSignal::LocalTranscript;
                                    Some(found)
                                } else {
                                    // Terminal: explicit Absent → reject (precision).
                                    // Session is ending; no more audio for stage-2 retry.
                                    log::info!(
                                        "[wake-phrase] terminal stage2 Absent reject KWS embedded_session_id={} prior_absent_count={} (anti false-wake)",
                                        embedded_session_id,
                                        candidate.kws_local_absent_count
                                    );
                                    None
                                }
                            }
                            Ok(Err(err)) => {
                                if secondary_fallback_can_accept_keyword(
                                    true,
                                    candidate.kws_local_absent_count,
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
                                        candidate.kws_local_absent_count
                                    );
                                    None
                                }
                            }
                            Err(err) => {
                                if secondary_fallback_can_accept_keyword(
                                    true,
                                    candidate.kws_local_absent_count,
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
                                        candidate.kws_local_absent_count
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
                    if !should_run_terminal_offline_recall(
                        candidate.pcm.len(),
                        candidate.local_absent_count,
                    ) {
                        log::info!(
                            "[wake-phrase] terminal skip offline cascade: candidate too short embedded_session_id={} pcm_ms={} min_ms={}",
                            embedded_session_id,
                            candidate.pcm.len() / 32,
                            MIN_TERMINAL_OFFLINE_PCM_BYTES / 32
                        );
                        None
                    } else {
                        // Offline full-buffer gain + bounded cascade may recover gain-starved
                        // device candidates — but only as a provisional KeywordModel. Local
                        // transcript must still confirm the full wake phrase.
                        let offline_pcm = candidate.pcm.clone();
                        let offline_phrase = phrase.clone();
                        let offline_task = tauri::async_runtime::spawn_blocking(move || {
                            crate::wake_phrase::detect_with_recall_cascade(
                                &offline_pcm,
                                &offline_phrase,
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
                                    let confirm = spawn_local_wake_confirmation(
                                        inner,
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
                                                "[wake-phrase] terminal offline→local confirmation finished embedded_session_id={} matched={} phrase_relation={:?} snapshot_pcm_ms={} transcript_chars={} inference_ms={}",
                                                embedded_session_id,
                                                result.matched,
                                                result.phrase_relation,
                                                result.snapshot_pcm_ms,
                                                result.transcript_chars,
                                                result.inference_ms
                                            );
                                            if result.matched {
                                                phrase_signal = denzic_voice_activation_v1_core::PhraseSignal::LocalTranscript;
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
                                        let result = spawn_local_wake_confirmation(
                                            inner,
                                            candidate.pcm.clone(),
                                            phrase.clone(),
                                            false,
                                        )
                                        .await;
                                        match result {
                                            Ok(Ok(result)) => {
                                                local_confirmation_ms = local_confirmation_ms
                                                    .saturating_add(result.inference_ms);
                                                log::info!(
                                                    "[wake-phrase] terminal local confirmation finished embedded_session_id={} matched={} phrase_relation={:?} snapshot_pcm_ms={} transcript_chars={} inference_ms={}",
                                                    embedded_session_id,
                                                    result.matched,
                                                    result.phrase_relation,
                                                    result.snapshot_pcm_ms,
                                                    result.transcript_chars,
                                                    result.inference_ms
                                                );
                                                if result.matched
                                                    && local_confirmation_can_activate(
                                                        false,
                                                        result.phrase_relation,
                                                    )
                                                {
                                                    phrase_signal = denzic_voice_activation_v1_core::PhraseSignal::LocalTranscript;
                                                    let end_seconds =
                                                        refined_wake_end_seconds(
                                                            0.0,
                                                            &result,
                                                            phrase.chars().count(),
                                                        );
                                                    Some(crate::wake_phrase::Match {
                                                        end_seconds,
                                                        matched_keyword: None,
                                                    })
                                                } else {
                                                    None
                                                }
                                            }
                                            Ok(Err(err)) => {
                                                log::warn!(
                                                    "[wake-phrase] terminal local confirmation unavailable embedded_session_id={embedded_session_id}: {err}"
                                                );
                                                None
                                            }
                                            Err(err) => {
                                                log::warn!(
                                                    "[wake-phrase] terminal local confirmation task failed embedded_session_id={embedded_session_id}: {err}"
                                                );
                                                None
                                            }
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
                    reject_hidden_automatic_candidate(
                        "wake_phrase_detection_failed",
                        embedded_session_id,
                    );
                    return Ok(true);
                }
            };
            let voiceprint_pcm = candidate.pcm.clone();
            let voiceprint_phrase = phrase.clone();
            let verification_task = tauri::async_runtime::spawn_blocking(move || {
                let started = Instant::now();
                let result =
                    crate::speaker_verification::verify(&voiceprint_pcm, &voiceprint_phrase);
                (result, started.elapsed().as_millis() as u64)
            })
            .await;
            let (verification, voiceprint_ms) = match verification_task {
                Ok(result) => result,
                Err(err) => (Err(format!("声纹验证任务失败: {err}")), 0),
            };
            let total_ms = candidate
                .kws_total_ms
                .saturating_add(local_confirmation_ms)
                .saturating_add(voiceprint_ms);
            let gate_decision = denzic_voice_activation_v1_core::decide_gate(
                denzic_voice_activation_v1_core::GateInput {
                    phrase_signal: wake_match
                        .as_ref()
                        .map(|_| phrase_signal)
                        .unwrap_or(denzic_voice_activation_v1_core::PhraseSignal::None),
                    owner_match: verification.as_ref().ok().map(|result| result.matched),
                    terminal: true,
                },
            );
            log::info!(
                "[wake-phrase] automatic streaming gate embedded_session_id={} terminal=true pcm_ms={} kws_fed_bytes={} kws_ms={} local_confirmation_ms={} voiceprint_ms={} total_compute_ms={} phrase_signal={:?} gate_decision={:?} owner_matched={}",
                embedded_session_id,
                candidate.pcm.len() / 32,
                candidate.kws_fed_bytes,
                candidate.kws_total_ms,
                local_confirmation_ms,
                voiceprint_ms,
                total_ms,
                wake_match
                    .as_ref()
                    .map(|_| phrase_signal)
                    .unwrap_or(denzic_voice_activation_v1_core::PhraseSignal::None),
                gate_decision,
                verification.as_ref().is_ok_and(|result| result.matched)
            );
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
                reject_hidden_automatic_candidate(
                    "wake_phrase_non_match",
                    embedded_session_id,
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
                reject_hidden_automatic_candidate(reason, embedded_session_id);
                return Ok(true);
            }
            let result = match verification {
                Ok(result) => result,
                Err(_) => {
                    unreachable!("fail-closed speaker verification rejected the error above")
                }
            };
            log::info!(
                "[speaker-verification] automatic candidate decision embedded_session_id={} matched={} score={:.4} wake_phrase={} wake_end_s={:.3}",
                embedded_session_id,
                result.matched,
                result.score,
                phrase,
                wake_match.end_seconds
            );
            local_speaker_seed = Some((
                candidate.pcm.clone(),
                wake_match.end_seconds,
                phrase.clone(),
            ));
            save_bounded_wake_diagnostic(embedded_session_id, "accepted", &candidate.pcm);
            if phrase_signal == denzic_voice_activation_v1_core::PhraseSignal::KeywordModel {
                persist_verified_wake_phrase_calibration(phrase.clone()).await;
            }
            let post_wake_offset =
                post_wake_pcm_offset_bytes(wake_match.end_seconds, candidate.pcm.len());
            let post_wake_pcm_bytes = candidate.pcm.len().saturating_sub(post_wake_offset);
            let wake_anchor_offset =
                wake_speaker_anchor_pcm_offset_bytes(wake_match.end_seconds, candidate.pcm.len());
            candidate.pcm.drain(..wake_anchor_offset);
            // Terminal accept often happens after the device already auto-stopped.
            // Opening a host dictation session with <1s post-wake scrap produces
            // empty ASR + Error capsule (owner: completely unusable).
            const MIN_POST_WAKE_DICTATION_PCM_BYTES: usize = 16_000 * 2; // 1.0 s
            if post_wake_pcm_bytes < MIN_POST_WAKE_DICTATION_PCM_BYTES {
                log::info!(
                    "[wake-phrase] terminal accept dropped: post-wake pcm too short for host dictation embedded_session_id={} post_wake_pcm_ms={} min_ms={}",
                    embedded_session_id,
                    post_wake_pcm_bytes / 32,
                    MIN_POST_WAKE_DICTATION_PCM_BYTES / 32
                );
                if let Some(sid) = take_early_capsule_session_id(&mut candidate) {
                    dismiss_early_wake_recording_capsule(inner, sid);
                }
                save_bounded_wake_diagnostic(
                    embedded_session_id,
                    "wake-without-usable-dictation",
                    &[],
                );
                reject_hidden_automatic_candidate(
                    "wake_phrase_without_usable_dictation",
                    embedded_session_id,
                );
                return Ok(true);
            }
        } else {
            log::info!(
                "[speaker-verification] physical recording bypass embedded_session_id={embedded_session_id}"
            );
        }

        let mut session = begin_embedded_audio_dictation_session(inner).await?;
        if let Some((wake_pcm, wake_end_seconds, wake_phrase)) = local_speaker_seed {
            session.start_local_speaker_tracking(wake_pcm, wake_end_seconds, wake_phrase);
        }
        if !activate_embedded_audio_dictation_session(inner, session.session_id, 0.0) {
            return Err("嵌入式音频听写会话已被取消".to_string());
        }
        if automatic {
            let capsule_audio_ms = (candidate.pcm.len() / 32) as u64;
            let phrase = inner.prefs.get().voice_wake_phrase;
            arm_automatic_wake_text_guard(inner, session.session_id, phrase, capsule_audio_ms);
        }
        crate::observability::begin_embedded_audio_session(session.session_id, embedded_session_id);
        self.session = Some(session);
        let session = self
            .session
            .as_mut()
            .ok_or_else(|| "嵌入式音频流式听写 session 尚未创建".to_string())?;
        crate::observability::record_embedded_audio_first_packet(session.session_id);
        for chunk in candidate.pcm.chunks(EMBEDDED_AUDIO_FEED_CHUNK_BYTES) {
            session.consume_streaming_pcm(inner, chunk, None)?;
        }
        log::info!(
            "[speaker-verification] released buffered candidate to ASR embedded_session_id={} pcm_bytes={}",
            embedded_session_id,
            candidate.pcm.len()
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
        if !take_hidden_automatic_candidate_promotion() {
            self.speaker_candidate = Some(candidate);
            return Ok(false);
        }
        if self.embedded_session_id != Some(embedded_session_id) {
            return Err(format!(
                "物理录音接管 session 不一致: current={:?}, incoming={embedded_session_id}",
                self.embedded_session_id
            ));
        }

        let discarded_pcm_bytes = discard_pre_press_candidate_pcm(&mut candidate.pcm);
        let session = begin_embedded_audio_dictation_session(inner).await?;
        if !activate_embedded_audio_dictation_session(inner, session.session_id, 0.0) {
            return Err("物理录音接管会话已被取消".to_string());
        }
        crate::observability::begin_embedded_audio_session(session.session_id, embedded_session_id);
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
        let phrase = inner.prefs.get().voice_wake_phrase;
        let (wake_match, phrase_signal, local_confirmation_ms, kws_step_ms) = if let Some(pending) =
            pending_phrase_match
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
                    candidate.kind = BufferedSpeakerCandidateKind::Rejected;
                    candidate.pcm.clear();
                    clear_hidden_automatic_candidate();
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
                let feed_start = rotation_start_bytes.unwrap_or(candidate.kws_fed_bytes);
                let new_pcm = candidate.pcm[feed_start..].to_vec();
                let incremental_pcm = candidate.pcm[candidate.kws_fed_bytes..].to_vec();
                candidate.kws_fed_bytes = candidate.pcm.len();
                (
                    detector,
                    new_pcm,
                    incremental_pcm,
                    candidate.kws_stream_origin_bytes,
                    rotation_start_bytes,
                )
            };
            let phrase_for_rotation = phrase.clone();
            let wake_task = tauri::async_runtime::spawn_blocking(move || {
                let started = Instant::now();
                let (mut detector, stream_origin_bytes, rotated, pcm_to_feed) =
                    if let Some(rotation_start_bytes) = rotation_start_bytes {
                        match crate::wake_phrase::StreamingDetector::new(&phrase_for_rotation) {
                            Ok(fresh) => (fresh, rotation_start_bytes, true, new_pcm),
                            Err(err) => {
                                log::warn!(
                                    "[wake-phrase] rolling detector refresh failed; preserving current stream: {err}"
                                );
                                (detector, prior_origin_bytes, false, incremental_pcm)
                            }
                        }
                    } else {
                        (detector, prior_origin_bytes, false, new_pcm)
                    };
                let result = detector.accept_pcm(&pcm_to_feed);
                (
                    detector,
                    result,
                    started.elapsed().as_millis() as u64,
                    stream_origin_bytes,
                    rotated,
                )
            })
            .await;
            let (detector, wake_match, kws_step_ms, stream_origin_bytes, rotated) = match wake_task {
                Ok(result) => result,
                Err(err) => {
                    if let Some(candidate) = self.speaker_candidate.as_mut() {
                        candidate.kind = BufferedSpeakerCandidateKind::Rejected;
                        candidate.pcm.clear();
                    }
                    clear_hidden_automatic_candidate();
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
                    ) {
                        candidate.local_confirmation_window_origin_bytes = stream_origin_bytes;
                        candidate.local_confirmation_attempts = 0;
                        candidate.local_confirmation_last_snapshot_bytes = 0;
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
                        candidate.kind = BufferedSpeakerCandidateKind::Rejected;
                        candidate.pcm.clear();
                        clear_hidden_automatic_candidate();
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
                            let ladder_snapshot = local_confirmation_snapshot_for_window(
                                candidate.pcm.len(),
                                window_origin_bytes,
                                candidate.local_confirmation_attempts,
                            );
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
                            if ladder_snapshot.is_some() || kws_immediate || kws_retry {
                                if ladder_snapshot.is_some() {
                                    candidate.local_confirmation_attempts += 1;
                                }
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
                                candidate.local_confirmation_task =
                                    Some(spawn_local_wake_confirmation(
                                        inner,
                                        confirmation_pcm,
                                        phrase.clone(),
                                        kws_hit.is_none(),
                                    ));
                                log::info!(
                                        "[wake-phrase] stage2 local confirm started embedded_session_id={} attempt={} threshold_pcm_ms={} snapshot_pcm_ms={} window_origin_pcm_ms={} window_pcm_ms={} kws_hit={} kws_immediate={} kws_retry={}",
                                        embedded_session_id,
                                        candidate.local_confirmation_attempts,
                                        threshold_pcm_ms,
                                        snapshot_pcm_ms,
                                        task_origin_bytes / 32,
                                        confirmation_pcm_ms,
                                        kws_hit.is_some(),
                                        kws_immediate,
                                        kws_retry
                                    );
                            }
                        }
                        if candidate
                            .local_confirmation_task
                            .as_ref()
                            .is_some_and(|task| task.inner().is_finished())
                        {
                            candidate.local_confirmation_task.take().map(|task| {
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
                        if local_confirmation_task_is_stale(
                            task_origin_bytes,
                            current_window_origin_bytes,
                            task_has_keyword_model_hit,
                        ) {
                            log::info!(
                                "[wake-phrase] stale local confirmation discarded embedded_session_id={} task_origin_pcm_ms={} current_origin_pcm_ms={}",
                                embedded_session_id,
                                task_origin_bytes / 32,
                                current_window_origin_bytes / 32
                            );
                            None
                        } else {
                        match task_result {
                            Ok(Ok(result)) => {
                                local_confirmation_ms = result.inference_ms;
                                log::info!(
                                        "[wake-phrase] stage2 local confirm finished embedded_session_id={} matched={} phrase_relation={:?} snapshot_pcm_ms={} transcript_chars={} inference_ms={} window_origin_pcm_ms={} kws_hit={}",
                                        embedded_session_id,
                                        result.matched,
                                        result.phrase_relation,
                                        result.snapshot_pcm_ms,
                                        result.transcript_chars,
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
                                        end_seconds: refined_end,
                                        matched_keyword: kws_hit
                                            .and_then(|found| found.matched_keyword),
                                    })
                                } else if !result.matched {
                                    let (local_absent_count, kws_absent_count, counted_kws_absent) = {
                                        let candidate = self
                                            .speaker_candidate
                                            .as_mut()
                                            .ok_or_else(|| "自动唤醒候选已丢失".to_string())?;
                                        candidate.local_absent_count =
                                            candidate.local_absent_count.saturating_add(1);
                                        let mut counted_kws_absent = false;
                                        if task_has_keyword_model_hit {
                                            let authoritative_full_absent =
                                                completed_secondary_absent_is_authoritative(
                                                    result.phrase_relation,
                                                    result.transcript_chars,
                                                    phrase.chars().count(),
                                                );
                                            if authoritative_full_absent {
                                                candidate.kws_local_absent_count = candidate
                                                    .kws_local_absent_count
                                                    .max(1);
                                                counted_kws_absent = true;
                                                log::info!(
                                                    "[wake-phrase] stage2 full-length Absent authoritative embedded_session_id={} transcript_chars={} phrase_chars={} first_hit_pcm_ms={:?}",
                                                    embedded_session_id,
                                                    result.transcript_chars,
                                                    phrase.chars().count(),
                                                    candidate.kws_first_hit_pcm_ms
                                                );
                                            } else if kws_absent_counts_toward_reject(
                                                candidate.kws_first_hit_pcm_ms,
                                                result.snapshot_pcm_ms,
                                            ) {
                                                candidate.kws_local_absent_count = candidate
                                                    .kws_local_absent_count
                                                    .saturating_add(1);
                                                counted_kws_absent = true;
                                            } else {
                                                log::info!(
                                                    "[wake-phrase] stage2 early Absent held (phrase horizon) embedded_session_id={} snapshot_pcm_ms={} first_hit_pcm_ms={:?} min_post_hit_ms={}",
                                                    embedded_session_id,
                                                    result.snapshot_pcm_ms,
                                                    candidate.kws_first_hit_pcm_ms,
                                                    KWS_ABSENT_COUNT_MIN_POST_HIT_MS
                                                );
                                            }
                                        }
                                        (
                                            candidate.local_absent_count,
                                            candidate.kws_local_absent_count,
                                            counted_kws_absent,
                                        )
                                    };
                                    if !task_has_keyword_model_hit {
                                        let pcm_ms = self
                                            .speaker_candidate
                                            .as_ref()
                                            .map(|c| c.pcm.len() / 32)
                                            .unwrap_or(0);
                                        log::info!(
                                            "[wake-phrase] local-only Absent recorded embedded_session_id={} count={} pcm_ms={}",
                                            embedded_session_id,
                                            local_absent_count,
                                            pcm_ms
                                        );
                                        // Do NOT midstream-abort on exploratory Absents.
                                        // Owner evidence 2026-07-30: real 「开始录音」 can
                                        // get stage-1 KWS only at ~2.7 s; aborting at 2.4 s
                                        // (count=4 Absent) killed those wakes with zero KWS
                                        // hit. Firmware already caps hidden VA at ~4.5 s;
                                        // Type host VREC:STOP on terminal reject is enough.
                                    } else if counted_kws_absent
                                        && kws_absent_count >= KWS_SECONDARY_ABSENT_REJECT_COUNT
                                    {
                                        log::info!(
                                            "[wake-phrase] stage2 Absent reject embedded_session_id={} count={} (anti half-phrase false wake)",
                                            embedded_session_id,
                                            kws_absent_count
                                        );
                                    } else if counted_kws_absent {
                                        log::info!(
                                            "[wake-phrase] stage2 Absent retry embedded_session_id={} count={}/{}",
                                            embedded_session_id,
                                            kws_absent_count,
                                            KWS_SECONDARY_ABSENT_REJECT_COUNT
                                        );
                                    }
                                    None
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
                                        .map(|candidate| candidate.kws_local_absent_count)
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
                                    .map(|candidate| candidate.kws_local_absent_count)
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
                        let waited_ms = self
                            .speaker_candidate
                            .as_ref()
                            .and_then(|c| c.kws_first_hit_at)
                            .map(|t| t.elapsed().as_millis() as u64)
                            .unwrap_or(0);
                        let explicit_absent_count = self
                            .speaker_candidate
                            .as_ref()
                            .map(|candidate| candidate.kws_local_absent_count)
                            .unwrap_or(0);
                        if waited_ms >= KWS_SECONDARY_CONFIRM_BUDGET_MS
                            && secondary_fallback_can_accept_keyword(
                                true,
                                explicit_absent_count,
                            )
                        {
                            // Secondary slow/hung before returning evidence: fail-open
                            // so a broken helper cannot disable voice activation.
                            phrase_signal =
                                denzic_voice_activation_v1_core::PhraseSignal::KeywordModel;
                            log::info!(
                                "[wake-phrase] stage2 timeout fail-open KeywordModel embedded_session_id={} waited_ms={} budget_ms={}",
                                embedded_session_id,
                                waited_ms,
                                KWS_SECONDARY_CONFIRM_BUDGET_MS
                            );
                            Some(kws)
                        } else if waited_ms >= KWS_SECONDARY_CONFIRM_BUDGET_MS
                            && explicit_absent_count > 0
                        {
                            log::info!(
                                "[wake-phrase] stage2 timeout held after explicit Absent embedded_session_id={} waited_ms={} budget_ms={} absent_count={}",
                                embedded_session_id,
                                waited_ms,
                                KWS_SECONDARY_CONFIRM_BUDGET_MS,
                                explicit_absent_count
                            );
                            None
                        } else {
                            // Within budget: wait for stage-2 (do not bare-KWS Accept).
                            None
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
                });
                return Ok(false);
            }
            (
                wake_match,
                phrase_signal,
                local_confirmation_ms,
                kws_step_ms,
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
        let verification_task = tauri::async_runtime::spawn_blocking(move || {
            let started = Instant::now();
            let result = crate::speaker_verification::verify(&pcm, &voiceprint_phrase);
            (result, started.elapsed().as_millis() as u64)
        })
        .await;
        let (verification, voiceprint_ms) = match verification_task {
            Ok(result) => result,
            Err(err) => (Err(format!("声纹验证任务失败: {err}")), 0),
        };
        let total_ms = kws_ms
            .saturating_add(local_confirmation_ms)
            .saturating_add(voiceprint_ms);
        let gate_decision = denzic_voice_activation_v1_core::decide_gate(
            denzic_voice_activation_v1_core::GateInput {
                phrase_signal,
                owner_match: verification.as_ref().ok().map(|result| result.matched),
                terminal: false,
            },
        );
        log::info!(
            "[wake-phrase] automatic streaming gate embedded_session_id={} terminal=false pcm_ms={} kws_fed_bytes={} kws_step_ms={} kws_ms={} local_confirmation_ms={} voiceprint_ms={} total_compute_ms={} phrase_signal={:?} gate_decision={:?} owner_matched={}",
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
            verification.as_ref().is_ok_and(|result| result.matched)
        );

        if gate_decision != denzic_voice_activation_v1_core::GateDecision::Accept {
            if let Ok(result) = &verification {
                if !result.matched {
                    if let Some(retry_ms) = next_owner_verification_retry_ms(pcm_ms) {
                        candidate.pending_phrase_match = Some(PendingAutomaticPhraseMatch {
                            wake_match,
                            phrase_signal,
                            local_confirmation_ms,
                            owner_verification_start_ms: retry_ms,
                        });
                        log::info!(
                            "[wake-phrase] phrase hit retained for owner retry embedded_session_id={} pcm_ms={} next_owner_window_ms={} score={}",
                            embedded_session_id,
                            pcm_ms,
                            retry_ms,
                            result.score
                        );
                        return Ok(false);
                    }
                }
            }
            save_bounded_wake_diagnostic(
                embedded_session_id,
                "voiceprint-non-match",
                &candidate.pcm,
            );
            if let Some(candidate) = self.speaker_candidate.as_mut() {
                candidate.kind = BufferedSpeakerCandidateKind::Rejected;
                candidate.pcm.clear();
            }
            clear_hidden_automatic_candidate();
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

        let recording_control_task = tauri::async_runtime::spawn_blocking(|| {
            let started = Instant::now();
            let result =
                crate::embedded_ble::send_recording_control_activate(Duration::from_secs(2));
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
        );
        save_bounded_wake_diagnostic(embedded_session_id, "accepted", &candidate.pcm);
        clear_hidden_automatic_candidate();
        // Keep only the bounded tail containing the accepted wake phrase. This
        // gives cloud diarization a target-speaker anchor without sending the
        // earlier ambient candidate; the wake-text guard keeps it out of UI/output.
        let post_wake_offset =
            post_wake_pcm_offset_bytes(wake_match.end_seconds, candidate.pcm.len());
        let post_wake_pcm_bytes = candidate.pcm.len().saturating_sub(post_wake_offset);
        let wake_anchor_offset =
            wake_speaker_anchor_pcm_offset_bytes(wake_match.end_seconds, candidate.pcm.len());
        candidate.pcm.drain(..wake_anchor_offset);
        let capsule_request_ms = candidate.started_at.elapsed().as_millis() as u64;
        let latency = denzic_observability_v1_core::assess_duration_ms(
            capsule_request_ms,
            denzic_observability_v1_core::PerformanceBudget {
                target_ms: 1_200,
                ceiling_ms: 1_500,
            },
        );
        let latency_target_pass = latency.target_pass;
        let latency_ceiling_pass = latency.ceiling_pass;
        let phrase_tail_to_capsule_ms =
            wake_phrase_tail_to_capsule_ms(wake_match.end_seconds, capsule_request_ms);
        let phrase_tail_latency = denzic_observability_v1_core::assess_duration_ms(
            phrase_tail_to_capsule_ms,
            denzic_observability_v1_core::PerformanceBudget {
                target_ms: 350,
                ceiling_ms: 500,
            },
        );
        let mut session = begin_embedded_audio_dictation_session(inner).await?;
        session.start_local_speaker_tracking(
            local_speaker_seed.0,
            local_speaker_seed.1,
            local_speaker_seed.2,
        );
        if !activate_embedded_audio_dictation_session(inner, session.session_id, 0.0) {
            return Err("嵌入式音频听写会话已被取消".to_string());
        }
        let capsule_audio_ms = (candidate.pcm.len() / 32) as u64;
        arm_automatic_wake_text_guard(inner, session.session_id, phrase.clone(), capsule_audio_ms);
        crate::observability::begin_embedded_audio_session(session.session_id, embedded_session_id);
        self.session = Some(session);
        let session = self
            .session
            .as_mut()
            .ok_or_else(|| "嵌入式音频流式听写 session 尚未创建".to_string())?;
        crate::observability::record_embedded_audio_first_packet(session.session_id);
        for pcm in candidate.pcm.chunks(EMBEDDED_AUDIO_FEED_CHUNK_BYTES) {
            session.consume_streaming_pcm(inner, pcm, None)?;
        }
        let recording_control_ms = match recording_control_task.await {
            Ok((Ok(()), elapsed_ms)) => elapsed_ms,
            Ok((Err(err), elapsed_ms)) => {
                log::warn!(
                    "[embedded-ble] accepted automatic recording LED activation failed after capsule release embedded_session_id={} elapsed_ms={}: {}",
                    embedded_session_id,
                    elapsed_ms,
                    err
                );
                elapsed_ms
            }
            Err(err) => {
                log::warn!(
                    "[embedded-ble] accepted automatic recording LED activation task failed after capsule release embedded_session_id={embedded_session_id}: {err}"
                );
                0
            }
        };
        log::info!(
            "[wake-phrase] live automatic session activated and released embedded_session_id={} phrase={} phrase_signal={:?} wake_end_s={:.3} post_wake_pcm_bytes={} kws_ms={} local_confirmation_ms={} voiceprint_ms={} gate_total_ms={} recording_control_ms={} wake_to_capsule_request_ms={} latency_target_ms=1200 latency_target_pass={} latency_ceiling_ms=1500 latency_ceiling_pass={} phrase_tail_to_capsule_ms={} phrase_tail_target_ms=350 phrase_tail_target_pass={} phrase_tail_ceiling_ms=500 phrase_tail_ceiling_pass={}",
            embedded_session_id,
            phrase,
            phrase_signal,
            wake_match.end_seconds,
            post_wake_pcm_bytes,
            kws_ms,
            local_confirmation_ms,
            voiceprint_ms,
            total_ms,
            recording_control_ms,
            capsule_request_ms,
            latency_target_pass,
            latency_ceiling_pass,
            phrase_tail_to_capsule_ms,
            phrase_tail_latency.target_pass,
            phrase_tail_latency.ceiling_pass
        );
        Ok(true)
    }
}
