impl EmbeddedStreamingDictation {
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
            // 2026-08-09 12:46:59 激活竞态：听写会话在等激活后的新设备段时，
            // 新段（唤醒监听窗 rotation）直接绑定给听写，而不是报 session 不一致。
            if let Some((pre_segment_id, _)) = self.activation_segment_race_guard {
                if self.session.is_some()
                    && embedded_session_id != pre_segment_id
                    && start_origin == crate::embedded_audio::SessionStartOrigin::VoiceActivation
                {
                    if let Some(session_id) = self.session.as_ref().map(|session| session.session_id)
                    {
                        clear_embedded_ble_awaiting_post_activation_segment(inner, session_id);
                    }
                    self.embedded_session_id = Some(embedded_session_id);
                    self.activation_segment_race_guard = None;
                    log::info!(
                        "[coord] dictation session bound to post-activation embedded segment embedded_session_id={embedded_session_id} (pre-activation segment {pre_segment_id} did not finalize)"
                    );
                    return Ok(());
                }
            }
            if self.embedded_session_id != Some(embedded_session_id) {
                return Err(format!(
                    "嵌入式音频流式 session 不一致: current={:?}, incoming={embedded_session_id}",
                    self.embedded_session_id
                ));
            }
            return Ok(());
        }
        // A terminal wake can finish after the firmware has already closed the
        // wake segment. The host then opens a real dictation session and asks
        // the device for a fresh segment. That fresh segment still reports
        // VoiceActivation, but it is body audio for the already-bound session,
        // not another hidden wake candidate. Route it before the ordinary
        // origin/enrollment classifier; otherwise the visible capsule remains
        // stuck in Starting while every body segment is re-run through KWS.
        if let Some(coordinator_session_id) =
            terminal_wake_continuation_waiting_for_audio(inner)
        {
            log::info!(
                "[wake-phrase] attaching fresh embedded segment as terminal wake body embedded_session_id={embedded_session_id} coordinator_session_id={coordinator_session_id}"
            );
            self.begin_session_if_needed(inner, embedded_session_id)
                .await?;
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
                let mut lifecycle = inner.recording_lifecycle.lock();
                if !lifecycle.begin_candidate(embedded_session_id) {
                    return Err(format!(
                        "录音生命周期拒绝新的唤醒候选 embedded_session_id={embedded_session_id}"
                    ));
                }
                if !lifecycle.hidden_candidate_active() {
                    log::info!(
                        "[speaker-verification] device-key takeover pending bound to hidden candidate embedded_session_id={embedded_session_id}"
                    );
                }
            }
            // Do NOT await StreamingDetector::new here — it costs ~0.5–1s wall time and
            // delayed the first PCM into the buffer until after the user finished 开始录音.
            // Spawn init in the background; PCM appends immediately; KWS starts when ready.
            let wake_detector_init =
                if candidate_kind == BufferedSpeakerCandidateKind::Verification {
                    let phrase = inner.prefs.get().voice_wake_phrase;
                    Some(tauri::async_runtime::spawn_blocking(move || {
                        // Primary wake uses StreamingDetector::new() (short-prefix variants
                        // + recall-first bootstrap threshold) for weak speech / light slur.
                        crate::wake_phrase::StreamingDetector::new(&phrase)
                    }))
                } else {
                    None
                };
            log::info!(
                "[speaker-verification] buffering embedded candidate kind={candidate_kind:?} embedded_session_id={embedded_session_id} detector_deferred={}",
                wake_detector_init.is_some()
            );
            self.speaker_candidate = Some(BufferedSpeakerCandidate {
                kind: candidate_kind,
                pcm: Vec::new(),
                wake_detector: None,
                wake_detector_init,
                pending_phrase_match: None,
                owner_ambiguous_confirmations: 0,
                owner_best_ambiguous_score: 0.0,
                owner_verification_task: None,
                owner_verification_attempted: false,
                #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
                target_wake_extraction_task: None,
                #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
                target_wake_extraction_attempted: false,
                #[cfg(target_os = "windows")]
                local_confirmation_task: None,
                #[cfg(target_os = "windows")]
                local_confirmation_task_started_at: None,
                #[cfg(target_os = "windows")]
                local_confirmation_window_origin_bytes: 0,
                #[cfg(target_os = "windows")]
                local_confirmation_task_origin_bytes: 0,
                #[cfg(target_os = "windows")]
                local_confirmation_task_has_keyword_model_hit: false,
                #[cfg(target_os = "windows")]
                local_confirmation_prefix_retry: LocalConfirmationPrefixRetryState::default(),
                local_confirmation_attempts: 0,
                local_confirmation_last_snapshot_bytes: 0,
                kws_prompted_local_confirm: false,
                kws_local_absent_count: 0,
                local_absent_count: 0,
                local_kws_fusion_evidence: false,
                kws_phrase_detected: false,
                local_owner_overlap_near_confirmations: 0,
                local_partial_phrase_confirmations: 0,
                owner_near_phrase_confirmations: 0,
                wake_interference_baseline: WakeInterferenceBaseline::default(),
                #[cfg(target_os = "windows")]
                local_absent_coverage: None,
                kws_first_hit_at: None,
                kws_first_hit_pcm_ms: None,
                early_capsule_session_id: None,
                early_capsule_request_ms: None,
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
}
