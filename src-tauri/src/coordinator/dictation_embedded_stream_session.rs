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
        let mut session = begin_embedded_audio_dictation_session(inner).await?;
        let terminal_wake_continuation =
            take_terminal_wake_continuation(inner, session.session_id);
        if let Some(continuation) = terminal_wake_continuation.as_ref() {
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
        if !activate_embedded_audio_dictation_session(inner, session.session_id, 0.0) {
            if terminal_wake_continuation.is_some() {
                clear_automatic_wake_text_guard(inner);
            }
            return Err("嵌入式音频听写会话已被取消".to_string());
        }
        if !inner
            .recording_lifecycle
            .lock()
            .begin_manual_owner(embedded_session_id, session.session_id)
        {
            return Err(format!(
                "录音生命周期拒绝主人会话 embedded_session_id={embedded_session_id} coordinator_session_id={}",
                session.session_id
            ));
        }
        crate::observability::begin_embedded_audio_session(session.session_id, embedded_session_id);
        log::info!(
            "[coord] embedded audio streaming dictation started (embedded_session_id={embedded_session_id}, coordinator_session_id={}, asr={}, terminal_wake_continuation={})",
            session.session_id,
            session.active_asr,
            terminal_wake_continuation.is_some()
        );
        self.session = Some(session);
        Ok(())
    }
}
