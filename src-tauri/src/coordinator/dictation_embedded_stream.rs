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

    /// A-desktop 诊断: 正式录音期把 PCM 喂给一个常驻 KWS,命中唤醒词只记日志、
    /// 不切分、不影响转写。目的是实测噪声中"录音内误命中唤醒词"的频率,
    /// 据此决定完整续录方案(命中→提前停+缓存PCM灌入新会话)是否可行。
    async fn feed_reactivation_detector(&mut self, inner: &Arc<Inner>, pcm: &[u8]) {
        let (already_triggered, needs_create) = match self.session.as_ref() {
            Some(session) => (
                session.reactivation_triggered,
                session.reactivation_detector.is_none(),
            ),
            None => return,
        };
        if already_triggered {
            return;
        }
        if needs_create {
            let phrase = inner.prefs.get().voice_wake_phrase.clone();
            match tauri::async_runtime::spawn_blocking(move || {
                // A-desktop 诊断:用严格 detector(不生成"去首字/去末字"前缀变体),
                // 避免正式录音中"开始录像/录入"等近似音误命中。TTS 实测严格模式 0/10
                // 误命中(宽松 4/10)。真人场景误命中率以本诊断版实测 [A-desktop-diag] 为准。
                crate::wake_phrase::StreamingDetector::new_strict(&phrase)
            })
            .await
            {
                Ok(Ok(detector)) => {
                    if let Some(session) = self.session.as_mut() {
                        session.reactivation_detector = Some(detector);
                    }
                }
                Ok(Err(err)) => {
                    log::warn!("[A-desktop-diag] reactivation detector init failed: {err}");
                    return;
                }
                Err(err) => {
                    log::warn!("[A-desktop-diag] reactivation detector init task failed: {err}");
                    return;
                }
            }
        }
        let Some(mut detector) = self
            .session
            .as_mut()
            .and_then(|session| session.reactivation_detector.take())
        else {
            return;
        };
        let pcm_vec = pcm.to_vec();
        let accepted = tauri::async_runtime::spawn_blocking(move || {
            let result = detector.accept_pcm(&pcm_vec);
            (detector, result)
        })
        .await;
        match accepted {
            Ok((detector, Ok(Some(_)))) => {
                if let Some(session) = self.session.as_mut() {
                    session.reactivation_triggered = true;
                    session.reactivation_hit_count =
                        session.reactivation_hit_count.saturating_add(1);
                    log::info!(
                        "[A-desktop-diag] reactivation wake hit session_id={:?} hit_count={} (diagnostic only, transcript untouched)",
                        session.session_id,
                        session.reactivation_hit_count
                    );
                }
                drop(detector);
            }
            Ok((detector, Ok(None))) => {
                if let Some(session) = self.session.as_mut() {
                    session.reactivation_detector = Some(detector);
                }
            }
            Ok((detector, Err(err))) => {
                log::warn!("[A-desktop-diag] reactivation accept_pcm failed: {err}");
                drop(detector);
            }
            Err(err) => {
                log::warn!("[A-desktop-diag] reactivation accept_pcm task failed: {err}");
            }
        }
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
                if embedded_streaming_chunk_is_asr_input(&chunk) {
                    self.begin_session_if_needed(inner, chunk.session_id)
                        .await?;
                    self.feed_reactivation_detector(inner, &chunk.pcm).await;
                    let proactive_stop_due = {
                        let session = self
                            .session
                            .as_mut()
                            .ok_or_else(|| "嵌入式音频流式听写 session 尚未创建".to_string())?;
                        crate::observability::record_embedded_audio_first_packet(session.session_id);
                        session.consume_streaming_pcm(inner, &chunk.pcm)?;
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
                if let Some(session) = self.session.as_ref() {
                    crate::observability::record_embedded_audio_stop(session.session_id);
                }
                self.pending_stop_expected_packet_count = Some(expected_packet_count);
                self.show_transcribing_after_stop(inner);
                if self.collector.inner().has_successful_complete_session() {
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
                            reject_hidden_automatic_candidate("hidden_candidate_cancelled");
                        }
                        self.terminal_received = true;
                        return Ok(true);
                    }
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
        let mut kind = buffered_speaker_candidate_kind(
            start_origin,
            crate::speaker_verification::take_enrollment_arm(),
            crate::speaker_verification::is_enrolled(),
        );
        if let Some(mut candidate_kind) = kind.take() {
            let wake_detector = if candidate_kind == BufferedSpeakerCandidateKind::Verification {
                let phrase = inner.prefs.get().voice_wake_phrase;
                match tauri::async_runtime::spawn_blocking(move || {
                    // Primary wake uses StreamingDetector::new() (short-prefix variants
                    // + bootstrap threshold 0.08) for recall in noise / light slur.
                    // False starts are gated by owner voiceprint when enrolled, and by
                    // local paraformer confirmation (PresentLater allowed). new_strict
                    // is not used on this path — it was cutting real wake hits.
                    crate::wake_phrase::StreamingDetector::new(&phrase)
                })
                .await
                {
                    Ok(Ok(detector)) => Some(detector),
                    Ok(Err(err)) => {
                        log::warn!(
                            "[wake-phrase] hidden candidate rejected because streaming detector initialization failed embedded_session_id={embedded_session_id}: {err}"
                        );
                        candidate_kind = BufferedSpeakerCandidateKind::Rejected;
                        None
                    }
                    Err(err) => {
                        log::warn!(
                            "[wake-phrase] hidden candidate rejected because streaming detector task failed embedded_session_id={embedded_session_id}: {err}"
                        );
                        candidate_kind = BufferedSpeakerCandidateKind::Rejected;
                        None
                    }
                }
            } else {
                None
            };
            if candidate_kind == BufferedSpeakerCandidateKind::Verification {
                mark_hidden_automatic_candidate_active();
            } else {
                clear_hidden_automatic_candidate();
            }
            log::info!(
                "[speaker-verification] buffering embedded candidate kind={candidate_kind:?} embedded_session_id={embedded_session_id}"
            );
            self.speaker_candidate = Some(BufferedSpeakerCandidate {
                kind: candidate_kind,
                pcm: Vec::new(),
                wake_detector,
                pending_phrase_match: None,
                #[cfg(target_os = "windows")]
                local_confirmation_task: None,
                local_confirmation_attempts: 0,
                local_confirmation_last_snapshot_bytes: 0,
                kws_fed_bytes: 0,
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
                "[coord] embedded audio streaming AGC summary (mode=voice_gated_fixed_session_gain, first_voiced_pcm_ms={:?}, voiced_chunks={}, quiet_chunks={}, observed_signal_rms_min={:?}, observed_signal_rms_max={:.2}, observed_signal_peak_max={}, pre_calibration_quiet_chunks={}, pre_calibration_signal_rms_max={:.2}, pre_calibration_signal_peak_max={}, first_eligible_signal_rms={:?}, first_eligible_signal_peak={:?}, first_gain={:?}, final_gain={:.2}, max_gain={:.2}, gain_updates={}, clipped_samples={})",
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
                session.streaming_agc.clipped_samples
            );
        }
        log::info!(
            "[coord] embedded audio streaming submitted to dictation pipeline (asr={}, input_mode={}, pcm_bytes={}, asr_pcm_bytes={}, archive={})",
            session.active_asr,
            if session.active_asr == "volcengine" {
                "voice_gated_fixed_session_gain"
            } else {
                "normalized_pcm"
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
            reject_hidden_automatic_candidate("automatic_candidate_rejected");
            return Ok(true);
        }
        if candidate.kind == BufferedSpeakerCandidateKind::Enrollment {
            let pcm = candidate.pcm;
            let phrase = inner.prefs.get().voice_wake_phrase;
            let result = tauri::async_runtime::spawn_blocking(move || {
                crate::wake_phrase::calibrate(&pcm, &phrase)?;
                crate::speaker_verification::finish_enrollment(&pcm)
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
        let mut automatic_wake_phrase = None;
        if automatic {
            let phrase = inner.prefs.get().voice_wake_phrase;
            let Some(mut detector) = candidate.wake_detector.take() else {
                save_bounded_wake_diagnostic(
                    embedded_session_id,
                    "detector-unavailable",
                    &candidate.pcm,
                );
                reject_hidden_automatic_candidate("wake_phrase_detection_failed");
                return Ok(true);
            };
            let remaining_pcm = candidate.pcm[candidate.kws_fed_bytes..].to_vec();
            candidate.kws_fed_bytes = candidate.pcm.len();
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
                    reject_hidden_automatic_candidate("wake_phrase_detection_failed");
                    return Ok(true);
                }
            };
            candidate.kws_total_ms = candidate.kws_total_ms.saturating_add(final_kws_ms);
            let mut phrase_signal = denzic_voice_activation_v1_core::PhraseSignal::KeywordModel;
            let mut local_confirmation_ms = 0u64;
            let wake_match = match wake_match {
                Ok(Some(found)) => Some(found),
                Ok(None) => {
                    // Streaming KWS missed — offline full-buffer gain + sensitive cascade
                    // recovers many device candidates where pre-roll locked gain too low.
                    let offline_pcm = candidate.pcm.clone();
                    let offline_phrase = phrase.clone();
                    let offline = tauri::async_runtime::spawn_blocking(move || {
                        crate::wake_phrase::detect_with_recall_cascade(&offline_pcm, &offline_phrase)
                    })
                    .await;
                    match offline {
                        Ok(Ok(Some(found))) => {
                            log::info!(
                                "[wake-phrase] terminal offline recall recovered embedded_session_id={} end_s={:.3}",
                                embedded_session_id,
                                found.end_seconds
                            );
                            Some(found)
                        }
                        Ok(Ok(None)) | Ok(Err(_)) | Err(_) => {
                            if let Ok(Err(err)) = &offline {
                                log::warn!(
                                    "[wake-phrase] terminal offline recall failed embedded_session_id={embedded_session_id}: {err}"
                                );
                            }
                            if let Err(err) = &offline {
                                log::warn!(
                                    "[wake-phrase] terminal offline recall task failed embedded_session_id={embedded_session_id}: {err}"
                                );
                            }
                            #[cfg(target_os = "windows")]
                            {
                                let mut task = candidate.local_confirmation_task.take();
                                let mut matched = None;
                                loop {
                                    if task.is_none()
                                        && candidate.local_confirmation_attempts
                                            < LOCAL_CONFIRMATION_SNAPSHOT_MS.len()
                                        && candidate.pcm.len() >= LOCAL_CONFIRMATION_START_BYTES
                                        && candidate.pcm.len()
                                            > candidate.local_confirmation_last_snapshot_bytes
                                    {
                                        candidate.local_confirmation_attempts += 1;
                                        candidate.local_confirmation_last_snapshot_bytes =
                                            candidate.pcm.len();
                                        task = Some(spawn_local_wake_confirmation(
                                            inner,
                                            candidate.pcm.clone(),
                                            phrase.clone(),
                                        ));
                                        log::info!(
                                            "[wake-phrase] terminal local confirmation started embedded_session_id={} attempt={} snapshot_pcm_ms={}",
                                            embedded_session_id,
                                            candidate.local_confirmation_attempts,
                                            candidate.pcm.len() / 32
                                        );
                                    }
                                    let Some(current_task) = task.take() else {
                                        break;
                                    };
                                    match current_task.await {
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
                                            if result.matched {
                                                phrase_signal = denzic_voice_activation_v1_core::PhraseSignal::LocalTranscript;
                                                matched = Some(crate::wake_phrase::Match {
                                                    end_seconds: 0.0,
                                                });
                                                break;
                                            }
                                        }
                                        Ok(Err(err)) => {
                                            log::warn!(
                                                "[wake-phrase] terminal local confirmation unavailable embedded_session_id={embedded_session_id}: {err}"
                                            );
                                        }
                                        Err(err) => {
                                            log::warn!(
                                                "[wake-phrase] terminal local confirmation task failed embedded_session_id={embedded_session_id}: {err}"
                                            );
                                        }
                                    }
                                }
                                matched
                            }
                            #[cfg(not(target_os = "windows"))]
                            {
                                None
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
                    reject_hidden_automatic_candidate("wake_phrase_detection_failed");
                    return Ok(true);
                }
            };
            let voiceprint_pcm = candidate.pcm.clone();
            let verification_task = tauri::async_runtime::spawn_blocking(move || {
                let started = Instant::now();
                let result = crate::speaker_verification::verify(&voiceprint_pcm);
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
                save_bounded_wake_diagnostic(
                    embedded_session_id,
                    "phrase-non-match",
                    &candidate.pcm,
                );
                reject_hidden_automatic_candidate("wake_phrase_non_match");
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
                save_bounded_wake_diagnostic(embedded_session_id, reason, &candidate.pcm);
                reject_hidden_automatic_candidate(reason);
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
            save_bounded_wake_diagnostic(embedded_session_id, "accepted", &candidate.pcm);
            if phrase_signal == denzic_voice_activation_v1_core::PhraseSignal::KeywordModel {
                persist_verified_wake_phrase_calibration(phrase.clone()).await;
            }
            let post_wake_offset =
                if phrase_signal == denzic_voice_activation_v1_core::PhraseSignal::KeywordModel {
                    ((wake_match.end_seconds + 0.12) * 32_000.0) as usize
                } else {
                    0
                };
            let post_wake_offset = post_wake_offset.min(candidate.pcm.len()) & !1usize;
            candidate.pcm.drain(..post_wake_offset);
            if candidate.pcm.is_empty() {
                reject_hidden_automatic_candidate("wake_phrase_without_dictation");
                return Ok(true);
            }
            automatic_wake_phrase = Some(phrase);
        } else {
            log::info!(
                "[speaker-verification] physical recording bypass embedded_session_id={embedded_session_id}"
            );
        }

        let session = begin_embedded_audio_dictation_session(inner).await?;
        if !activate_embedded_audio_dictation_session(inner, session.session_id, 0.0) {
            return Err("嵌入式音频听写会话已被取消".to_string());
        }
        if let Some(phrase) = automatic_wake_phrase {
            set_embedded_audio_wake_phrase_filter(inner, session.session_id, phrase);
        }
        crate::observability::begin_embedded_audio_session(session.session_id, embedded_session_id);
        self.session = Some(session);
        let session = self
            .session
            .as_mut()
            .ok_or_else(|| "嵌入式音频流式听写 session 尚未创建".to_string())?;
        crate::observability::record_embedded_audio_first_packet(session.session_id);
        for chunk in candidate.pcm.chunks(EMBEDDED_AUDIO_FEED_CHUNK_BYTES) {
            session.consume_streaming_pcm(inner, chunk)?;
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
            let (mut detector, new_pcm) = {
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
                let new_pcm = candidate.pcm[candidate.kws_fed_bytes..].to_vec();
                candidate.kws_fed_bytes = candidate.pcm.len();
                (detector, new_pcm)
            };
            let wake_task = tauri::async_runtime::spawn_blocking(move || {
                let started = Instant::now();
                let result = detector.accept_pcm(&new_pcm);
                (detector, result, started.elapsed().as_millis() as u64)
            })
            .await;
            let (detector, wake_match, kws_step_ms) = match wake_task {
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
                candidate.kws_total_ms = candidate.kws_total_ms.saturating_add(kws_step_ms);
                match wake_match {
                    Ok(found) => found,
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
            let mut phrase_signal = denzic_voice_activation_v1_core::PhraseSignal::KeywordModel;
            let mut local_confirmation_ms = 0u64;
            let wake_match = if wake_match.is_some() {
                wake_match
            } else {
                #[cfg(target_os = "windows")]
                {
                    let completed_task = {
                        let candidate = self
                            .speaker_candidate
                            .as_mut()
                            .ok_or_else(|| "自动唤醒候选已丢失".to_string())?;
                        if candidate.local_confirmation_task.is_none() {
                            if let Some(snapshot_bytes) = next_local_confirmation_snapshot_bytes(
                                candidate.local_confirmation_attempts,
                            )
                            .filter(|snapshot_bytes| candidate.pcm.len() >= *snapshot_bytes)
                            {
                                candidate.local_confirmation_attempts += 1;
                                candidate.local_confirmation_last_snapshot_bytes =
                                    candidate.pcm.len();
                                candidate.local_confirmation_task =
                                    Some(spawn_local_wake_confirmation(
                                        inner,
                                        candidate.pcm.clone(),
                                        phrase.clone(),
                                    ));
                                log::info!(
                                        "[wake-phrase] bounded local confirmation started embedded_session_id={} attempt={} threshold_pcm_ms={} snapshot_pcm_ms={}",
                                        embedded_session_id,
                                        candidate.local_confirmation_attempts,
                                        snapshot_bytes / 32,
                                        candidate.pcm.len() / 32
                                    );
                            }
                        }
                        if candidate
                            .local_confirmation_task
                            .as_ref()
                            .is_some_and(|task| task.inner().is_finished())
                        {
                            candidate.local_confirmation_task.take()
                        } else {
                            None
                        }
                    };
                    if let Some(task) = completed_task {
                        match task.await {
                            Ok(Ok(result)) => {
                                local_confirmation_ms = result.inference_ms;
                                log::info!(
                                        "[wake-phrase] bounded local confirmation finished embedded_session_id={} matched={} phrase_relation={:?} snapshot_pcm_ms={} transcript_chars={} inference_ms={}",
                                        embedded_session_id,
                                        result.matched,
                                        result.phrase_relation,
                                        result.snapshot_pcm_ms,
                                        result.transcript_chars,
                                        result.inference_ms
                                    );
                                if result.matched {
                                    phrase_signal = denzic_voice_activation_v1_core::PhraseSignal::LocalTranscript;
                                    Some(crate::wake_phrase::Match { end_seconds: 0.0 })
                                } else {
                                    None
                                }
                            }
                            Ok(Err(err)) => {
                                log::warn!(
                                        "[wake-phrase] bounded local confirmation unavailable embedded_session_id={embedded_session_id}: {err}"
                                    );
                                None
                            }
                            Err(err) => {
                                log::warn!(
                                        "[wake-phrase] bounded local confirmation task failed embedded_session_id={embedded_session_id}: {err}"
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
            };
            let Some(wake_match) = wake_match else {
                return Ok(false);
            };
            if self
                .speaker_candidate
                .as_ref()
                .is_some_and(|candidate| !owner_verification_window_ready(candidate.pcm.len()))
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
        let verification_task = tauri::async_runtime::spawn_blocking(move || {
            let started = Instant::now();
            let result = crate::speaker_verification::verify(&pcm);
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
        save_bounded_wake_diagnostic(embedded_session_id, "accepted", &candidate.pcm);
        clear_hidden_automatic_candidate();
        let post_wake_offset =
            if phrase_signal == denzic_voice_activation_v1_core::PhraseSignal::KeywordModel {
                (wake_match.end_seconds * 32_000.0) as usize
            } else {
                0
            };
        let post_wake_offset = post_wake_offset.min(candidate.pcm.len()) & !1usize;
        candidate.pcm.drain(..post_wake_offset);
        let capsule_request_ms = candidate.started_at.elapsed().as_millis() as u64;
        let latency_target_pass = capsule_request_ms <= 1_200;
        let latency_ceiling_pass = capsule_request_ms <= 1_500;
        let session = begin_embedded_audio_dictation_session(inner).await?;
        if !activate_embedded_audio_dictation_session(inner, session.session_id, 0.0) {
            return Err("嵌入式音频听写会话已被取消".to_string());
        }
        set_embedded_audio_wake_phrase_filter(inner, session.session_id, phrase.clone());
        crate::observability::begin_embedded_audio_session(session.session_id, embedded_session_id);
        self.session = Some(session);
        let session = self
            .session
            .as_mut()
            .ok_or_else(|| "嵌入式音频流式听写 session 尚未创建".to_string())?;
        crate::observability::record_embedded_audio_first_packet(session.session_id);
        for pcm in candidate.pcm.chunks(EMBEDDED_AUDIO_FEED_CHUNK_BYTES) {
            session.consume_streaming_pcm(inner, pcm)?;
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
            "[wake-phrase] live automatic session activated and released embedded_session_id={} phrase={} phrase_signal={:?} wake_end_s={:.3} post_wake_pcm_bytes={} kws_ms={} local_confirmation_ms={} voiceprint_ms={} gate_total_ms={} recording_control_ms={} wake_to_capsule_request_ms={} latency_target_ms=1200 latency_target_pass={} latency_ceiling_ms=1500 latency_ceiling_pass={}",
            embedded_session_id,
            phrase,
            phrase_signal,
            wake_match.end_seconds,
            candidate.pcm.len(),
            kws_ms,
            local_confirmation_ms,
            voiceprint_ms,
            total_ms,
            recording_control_ms,
            capsule_request_ms,
            latency_target_pass,
            latency_ceiling_pass
        );
        Ok(true)
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
        clear_hidden_automatic_candidate();
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
        if let Some(session) = self.session.take() {
            crate::observability::record_embedded_audio_failure(session.session_id, message);
            cancel_asr_for_session(inner, session.session_id);
            restore_prepared_windows_ime_session(inner, session.session_id);
            publish_dictation_pipeline_error(inner, session.session_id, message.to_string());
        } else {
            let elapsed = inner.state.lock().started_at.elapsed().as_millis() as u64;
            emit_capsule(
                inner,
                CapsuleState::Error,
                0.0,
                elapsed,
                Some(message.to_string()),
                None,
            );
        }
        schedule_capsule_idle(inner, CAPSULE_STREAM_ERROR_HIDE_DELAY_MS, event_session_id);
        self.terminal_received = true;
    }

    fn show_transcribing_after_stop(&self, inner: &Arc<Inner>) {
        if let Some(session) = self.session.as_ref() {
            let already_latched = embedded_audio_stop_feedback_latched(inner);
            latch_embedded_audio_stop_feedback(inner);
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
        clear_hidden_automatic_candidate();
        self.collector.reset();
        self.session = None;
        self.speaker_candidate = None;
        self.embedded_session_id = None;
        self.transcript = None;
        self.pending_stop_expected_packet_count = None;
        self.terminal_received = false;
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
