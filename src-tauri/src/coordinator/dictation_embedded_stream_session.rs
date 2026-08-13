// Embedded stream session attachment.
// Included into `coordinator::dictation` via `include!`.

impl EmbeddedStreamingDictation {
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
