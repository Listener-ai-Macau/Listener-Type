// Dictation event publish + device AI processing LED helpers.
// Included into `coordinator::dictation` via `include!`.

fn apply_and_publish_dictation_event(
    inner: &Arc<Inner>,
    event: DictationEvent,
    level: f32,
    message: Option<String>,
    inserted_chars: Option<u32>,
) -> bool {
    let transition = {
        let mut state = inner.state.lock();
        apply_dictation_event(&mut state, event)
    };
    publish_dictation_transition(inner, transition, level, message, inserted_chars)
}

fn apply_embedded_ble_session_actor_dictation_event(
    inner: &Arc<Inner>,
    command: EmbeddedBleSessionActorCommand,
    session_id: SessionId,
    detail: impl Into<String>,
    event: DictationEvent,
    level: f32,
    message: Option<String>,
    inserted_chars: Option<u32>,
) -> bool {
    apply_embedded_ble_session_actor_dictation_event_with_trace(
        inner,
        command,
        session_id,
        detail,
        true,
        event,
        level,
        message,
        inserted_chars,
    )
}

fn apply_embedded_ble_session_actor_dictation_event_with_trace(
    inner: &Arc<Inner>,
    command: EmbeddedBleSessionActorCommand,
    session_id: SessionId,
    detail: impl Into<String>,
    trace_timeline: bool,
    event: DictationEvent,
    level: f32,
    message: Option<String>,
    inserted_chars: Option<u32>,
) -> bool {
    dispatch_embedded_ble_session_actor_command_with_trace(
        inner,
        command,
        Some(session_id),
        detail,
        trace_timeline,
        |_| apply_and_publish_dictation_event(inner, event, level, message, inserted_chars),
    )
}

fn embedded_ble_actor_context_active(inner: &Arc<Inner>) -> bool {
    inner.embedded_ble_cancel_flag.lock().is_some() || inner.embedded_audio_stats.lock().is_some()
}

fn embedded_ble_host_recording_control_context_active(inner: &Arc<Inner>) -> bool {
    embedded_ble_actor_context_active(inner)
        || inner.prefs.get().dictation_input_source == DictationInputSource::EmbeddedBle
}

fn embedded_ble_host_cancel_context_active(inner: &Arc<Inner>) -> bool {
    if embedded_ble_actor_context_active(inner) {
        return true;
    }
    let phase = inner.state.lock().phase;
    inner.prefs.get().dictation_input_source == DictationInputSource::EmbeddedBle
        && matches!(
            phase,
            SessionPhase::Starting | SessionPhase::Listening | SessionPhase::Processing
        )
}

fn publish_dictation_pipeline_error(
    inner: &Arc<Inner>,
    session_id: SessionId,
    message: String,
) -> bool {
    apply_and_publish_dictation_event(
        inner,
        DictationEvent::PipelineError { session_id },
        0.0,
        Some(message),
        None,
    )
}

fn publish_dictation_timeout(inner: &Arc<Inner>, session_id: SessionId, message: String) -> bool {
    apply_and_publish_dictation_event(
        inner,
        DictationEvent::Timeout { session_id },
        0.0,
        Some(message),
        None,
    )
}

fn schedule_actionable_error_capsule_idle(inner: &Arc<Inner>, session_id: SessionId) {
    schedule_capsule_idle(
        inner,
        CAPSULE_ACTIONABLE_ERROR_HIDE_DELAY_MS,
        Some(session_id),
    );
}

fn schedule_empty_transcript_capsule_idle(inner: &Arc<Inner>, session_id: SessionId) {
    schedule_capsule_idle(
        inner,
        CAPSULE_EMPTY_TRANSCRIPT_HIDE_DELAY_MS,
        Some(session_id),
    );
}

fn publish_embedded_ble_asr_final(
    inner: &Arc<Inner>,
    session_id: SessionId,
    transcript_empty: bool,
    message: Option<String>,
) -> bool {
    let detail = format!("transcript_empty={transcript_empty}");
    let published = if embedded_ble_actor_context_active(inner) {
        apply_embedded_ble_session_actor_dictation_event(
            inner,
            EmbeddedBleSessionActorCommand::AsrFinal,
            session_id,
            detail,
            DictationEvent::AsrFinal {
                session_id,
                transcript_empty,
            },
            0.0,
            message,
            None,
        )
    } else {
        apply_and_publish_dictation_event(
            inner,
            DictationEvent::AsrFinal {
                session_id,
                transcript_empty,
            },
            0.0,
            message,
            None,
        )
    };
    if published {
        crate::observability::record_embedded_audio_final(session_id);
    }
    published
}

fn publish_embedded_ble_wake_only_expired(inner: &Arc<Inner>, session_id: SessionId) -> bool {
    let event = DictationEvent::WakeOnlyExpired { session_id };
    if embedded_ble_actor_context_active(inner) {
        apply_embedded_ble_session_actor_dictation_event(
            inner,
            EmbeddedBleSessionActorCommand::AsrFinal,
            session_id,
            "wake_only_expired transcript_empty=true",
            event,
            0.0,
            None,
            None,
        )
    } else {
        apply_and_publish_dictation_event(inner, event, 0.0, None, None)
    }
}

fn finish_dictation_pipeline_error(
    inner: &Arc<Inner>,
    session_id: SessionId,
    message: String,
) -> bool {
    let observability_message = message.clone();
    set_device_ai_processing_warning_async(
        inner,
        "dictation_pipeline_error",
        Duration::from_millis(0),
    );
    if !publish_dictation_pipeline_error(inner, session_id, message) {
        return false;
    }
    restore_prepared_windows_ime_session(inner, session_id);
    schedule_actionable_error_capsule_idle(inner, session_id);
    crate::observability::record_embedded_audio_failure(session_id, &observability_message);
    true
}

fn finish_dictation_timeout(inner: &Arc<Inner>, session_id: SessionId, message: String) -> bool {
    set_device_ai_processing_warning_async(inner, "dictation_timeout", Duration::from_millis(0));
    let published = if embedded_ble_actor_context_active(inner) {
        apply_embedded_ble_session_actor_dictation_event(
            inner,
            EmbeddedBleSessionActorCommand::Timeout,
            session_id,
            message.clone(),
            DictationEvent::Timeout { session_id },
            0.0,
            Some(message),
            None,
        )
    } else {
        publish_dictation_timeout(inner, session_id, message)
    };
    if !published {
        return false;
    }
    restore_prepared_windows_ime_session(inner, session_id);
    schedule_actionable_error_capsule_idle(inner, session_id);
    crate::observability::record_embedded_audio_timeout(session_id);
    true
}

fn clear_embedded_audio_stats(inner: &Arc<Inner>) {
    *inner.embedded_audio_stats.lock() = None;
}

fn begin_embedded_audio_preview_session(inner: &Arc<Inner>, session_id: SessionId) {
    inner.embedded_audio_preview.lock().begin_session(session_id);
    *inner.embedded_audio_last_capsule_level.lock() = 0.0;
}

fn clear_embedded_audio_preview_session(inner: &Arc<Inner>, session_id: SessionId) {
    if inner
        .embedded_audio_preview
        .lock()
        .clear_session(session_id)
    {
        *inner.embedded_audio_last_capsule_level.lock() = 0.0;
    }
}

fn current_embedded_audio_capsule_level(inner: &Arc<Inner>) -> f32 {
    *inner.embedded_audio_last_capsule_level.lock()
}

fn remember_embedded_audio_capsule_level(inner: &Arc<Inner>, level: f32) -> f32 {
    let clamped = level.clamp(0.0, 1.0);
    *inner.embedded_audio_last_capsule_level.lock() = clamped;
    clamped
}

fn clear_embedded_audio_stop_feedback(inner: &Arc<Inner>) {
    inner
        .embedded_audio_stop_feedback_latched
        .store(false, Ordering::SeqCst);
    *inner.dictation_stop_feedback_at.lock() = None;
}

fn latch_embedded_audio_stop_feedback(inner: &Arc<Inner>, session_id: SessionId) {
    inner
        .embedded_audio_stop_feedback_latched
        .store(true, Ordering::SeqCst);
    *inner.dictation_stop_feedback_at.lock() = Some((session_id, Instant::now()));
    // Keep the wake-prefix/body gate in lockstep with the visible stop
    // boundary. Otherwise a late provider final can promote a wake-only
    // capsule and inject text after the user has already stopped.
    mark_automatic_wake_stop_requested(inner, session_id);
}

fn embedded_audio_stop_feedback_latched(inner: &Arc<Inner>) -> bool {
    inner
        .embedded_audio_stop_feedback_latched
        .load(Ordering::SeqCst)
}

fn take_stop_to_done_ms(inner: &Arc<Inner>, session_id: SessionId) -> Option<u64> {
    let mut guard = inner.dictation_stop_feedback_at.lock();
    match *guard {
        Some((sid, started)) if sid == session_id => {
            let ms = started.elapsed().as_millis() as u64;
            *guard = None;
            Some(ms)
        }
        _ => None,
    }
}

fn embedded_ble_processing_sync_disabled() -> bool {
    std::env::var(EMBEDDED_BLE_DISABLE_PROCESSING_SYNC_ENV)
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

fn device_ai_processing_io_allowed() -> bool {
    !cfg!(test)
}

fn should_sync_device_ai_processing(inner: &Arc<Inner>) -> bool {
    device_ai_processing_io_allowed()
        && embedded_ble_host_recording_control_context_active(inner)
        && !embedded_ble_processing_sync_disabled()
}

fn set_device_ai_processing_async(inner: &Arc<Inner>, active: bool, reason: &'static str) {
    if !should_sync_device_ai_processing(inner) {
        return;
    }
    let session_id = inner.state.lock().session_id;
    async_runtime::spawn_blocking(move || {
        match crate::embedded_ble::send_recording_processing_state(active, Duration::from_secs(2)) {
            Ok(()) => log::info!(
                "[embedded-ble] device AI processing LED synced active={active} reason={reason} session_id={session_id}"
            ),
            Err(err) => log::warn!(
                "[embedded-ble] device AI processing LED sync failed active={active} reason={reason} session_id={session_id}: {err}"
            ),
        }
    });
}

fn device_ai_processing_completion_delay(started_at: Option<Instant>, now: Instant) -> Duration {
    let Some(started_at) = started_at else {
        return Duration::from_millis(0);
    };
    let min_visible = Duration::from_millis(DEVICE_AI_PROCESSING_MIN_VISIBLE_MS);
    min_visible.saturating_sub(now.saturating_duration_since(started_at))
}

fn schedule_device_ai_processing_max_visible_timeout(
    inner: &Arc<Inner>,
    cancel: Arc<AtomicBool>,
    reason: &'static str,
) {
    if !should_sync_device_ai_processing(inner) {
        return;
    }
    let expected_session_id = inner.state.lock().session_id;
    let inner = Arc::clone(inner);
    async_runtime::spawn_blocking(move || {
        std::thread::sleep(Duration::from_millis(DEVICE_AI_PROCESSING_MAX_VISIBLE_MS));
        if cancel.load(Ordering::SeqCst) {
            return;
        }
        let session_id = inner.state.lock().session_id;
        if session_id != expected_session_id {
            log::info!(
                "[embedded-ble] skipped stale device AI processing LED max-visible timeout reason={reason} expected_session_id={expected_session_id} current_session_id={session_id}"
            );
            return;
        }
        if !should_sync_device_ai_processing(&inner) {
            log::info!(
                "[embedded-ble] skipped device AI processing LED max-visible timeout reason={reason} session_id={session_id}; processing sync no longer active"
            );
            return;
        }
        // Cap purple AI only. Do NOT send PROCESSING:DONE here — DONE flashes green OK.
        // Long ASR/polish used to fire DONE at 5s and again at real completion → intermittent
        // double green flash after a recording finishes.
        match crate::embedded_ble::send_recording_processing_state(false, Duration::from_secs(2))
        {
            Ok(()) => log::warn!(
                "[embedded-ble] device AI processing LED max-visible timeout stopped reason={reason} session_id={session_id} max_visible_ms={DEVICE_AI_PROCESSING_MAX_VISIBLE_MS}"
            ),
            Err(err) => log::warn!(
                "[embedded-ble] device AI processing LED max-visible timeout stop failed reason={reason} session_id={session_id}: {err}"
            ),
        }
    });
}

fn set_device_ai_processing_done_async(inner: &Arc<Inner>, reason: &'static str, delay: Duration) {
    if !should_sync_device_ai_processing(inner) {
        return;
    }
    let expected_session_id = inner.state.lock().session_id;
    let inner = Arc::clone(inner);
    async_runtime::spawn_blocking(move || {
        if delay > Duration::from_millis(0) {
            std::thread::sleep(delay);
        }
        let session_id = inner.state.lock().session_id;
        if session_id != expected_session_id {
            log::info!(
                "[embedded-ble] skipped stale device AI processing LED completion reason={reason} expected_session_id={expected_session_id} current_session_id={session_id}"
            );
            return;
        }
        match crate::embedded_ble::send_recording_processing_done(Duration::from_secs(2)) {
            Ok(()) => log::info!(
                "[embedded-ble] device AI processing LED completed reason={reason} session_id={session_id} delayed_ms={}",
                delay.as_millis()
            ),
            Err(err) => log::warn!(
                "[embedded-ble] device AI processing LED completion sync failed reason={reason} session_id={session_id}: {err}"
            ),
        }
    });
}

async fn set_device_ai_processing_done_wait(
    inner: &Arc<Inner>,
    reason: &'static str,
    delay: Duration,
) {
    if !should_sync_device_ai_processing(inner) {
        return;
    }
    let expected_session_id = inner.state.lock().session_id;
    if delay > Duration::from_millis(0) {
        tokio::time::sleep(delay).await;
    }
    let session_id = inner.state.lock().session_id;
    if session_id != expected_session_id {
        log::info!(
            "[embedded-ble] skipped stale device AI processing LED completion reason={reason} expected_session_id={expected_session_id} current_session_id={session_id}"
        );
        return;
    }
    let result = async_runtime::spawn_blocking(move || {
        crate::embedded_ble::send_recording_processing_done(Duration::from_secs(2))
    })
    .await;
    match result {
        Ok(Ok(())) => log::info!(
            "[embedded-ble] device AI processing LED completed reason={reason} session_id={session_id} delayed_ms={}",
            delay.as_millis()
        ),
        Ok(Err(err)) => log::warn!(
            "[embedded-ble] device AI processing LED completion sync failed reason={reason} session_id={session_id}: {err}"
        ),
        Err(err) => log::warn!(
            "[embedded-ble] device AI processing LED completion task failed reason={reason} session_id={session_id}: {err}"
        ),
    }
}

fn set_device_ai_processing_warning_async(
    inner: &Arc<Inner>,
    reason: &'static str,
    delay: Duration,
) {
    if !should_sync_device_ai_processing(inner) {
        return;
    }
    let expected_session_id = inner.state.lock().session_id;
    let inner = Arc::clone(inner);
    async_runtime::spawn_blocking(move || {
        if delay > Duration::from_millis(0) {
            std::thread::sleep(delay);
        }
        let session_id = inner.state.lock().session_id;
        if session_id != expected_session_id {
            log::info!(
                "[embedded-ble] skipped stale device AI processing LED warning reason={reason} expected_session_id={expected_session_id} current_session_id={session_id}"
            );
            return;
        }
        match crate::embedded_ble::send_recording_processing_warning(Duration::from_secs(2)) {
            Ok(()) => log::info!(
                "[embedded-ble] device AI processing LED warning reason={reason} session_id={session_id} delayed_ms={}",
                delay.as_millis()
            ),
            Err(err) => log::warn!(
                "[embedded-ble] device AI processing LED warning sync failed reason={reason} session_id={session_id}: {err}"
            ),
        }
    });
}

async fn set_device_ai_processing_warning_wait(
    inner: &Arc<Inner>,
    reason: &'static str,
    delay: Duration,
) {
    if !should_sync_device_ai_processing(inner) {
        return;
    }
    let expected_session_id = inner.state.lock().session_id;
    if delay > Duration::from_millis(0) {
        tokio::time::sleep(delay).await;
    }
    let session_id = inner.state.lock().session_id;
    if session_id != expected_session_id {
        log::info!(
            "[embedded-ble] skipped stale device AI processing LED warning reason={reason} expected_session_id={expected_session_id} current_session_id={session_id}"
        );
        return;
    }
    let result = async_runtime::spawn_blocking(move || {
        crate::embedded_ble::send_recording_processing_warning(Duration::from_secs(2))
    })
    .await;
    match result {
        Ok(Ok(())) => log::info!(
            "[embedded-ble] device AI processing LED warning reason={reason} session_id={session_id} delayed_ms={}",
            delay.as_millis()
        ),
        Ok(Err(err)) => log::warn!(
            "[embedded-ble] device AI processing LED warning sync failed reason={reason} session_id={session_id}: {err}"
        ),
        Err(err) => log::warn!(
            "[embedded-ble] device AI processing LED warning task failed reason={reason} session_id={session_id}: {err}"
        ),
    }
}

fn device_ai_processing_completion_allowed(active: bool, completed: bool) -> bool {
    active && !completed
}

struct DeviceAiProcessingGuard {
    inner: Arc<Inner>,
    active: bool,
    completed: bool,
    started_at: Option<Instant>,
    max_visible_cancel: Option<Arc<AtomicBool>>,
}

impl DeviceAiProcessingGuard {
    fn defer(inner: &Arc<Inner>) -> Self {
        Self {
            inner: Arc::clone(inner),
            active: false,
            completed: false,
            started_at: None,
            max_visible_cancel: None,
        }
    }

    fn start_if_needed(&mut self, reason: &'static str) {
        if self.completed || self.active || !should_sync_device_ai_processing(&self.inner) {
            return;
        }
        set_device_ai_processing_async(&self.inner, true, reason);
        let cancel = Arc::new(AtomicBool::new(false));
        schedule_device_ai_processing_max_visible_timeout(
            &self.inner,
            Arc::clone(&cancel),
            "dictation_processing_max_visible_timeout",
        );
        self.max_visible_cancel = Some(cancel);
        self.active = true;
        self.started_at = Some(Instant::now());
    }

    fn cancel_max_visible_timeout(&mut self) {
        if let Some(cancel) = self.max_visible_cancel.take() {
            cancel.store(true, Ordering::SeqCst);
        }
    }

    async fn complete_success(&mut self, reason: &'static str) {
        if device_ai_processing_completion_allowed(self.active, self.completed) {
            self.cancel_max_visible_timeout();
            let delay = device_ai_processing_completion_delay(self.started_at, Instant::now());
            set_device_ai_processing_done_wait(&self.inner, reason, delay).await;
            self.completed = true;
            self.active = false;
            self.started_at = None;
        }
    }

    fn complete_success_async(&mut self, reason: &'static str) {
        if device_ai_processing_completion_allowed(self.active, self.completed) {
            self.cancel_max_visible_timeout();
            let delay = device_ai_processing_completion_delay(self.started_at, Instant::now());
            set_device_ai_processing_done_async(&self.inner, reason, delay);
            self.completed = true;
            self.active = false;
            self.started_at = None;
        }
    }

    async fn complete_warning(&mut self, reason: &'static str) {
        if device_ai_processing_completion_allowed(self.active, self.completed) {
            self.cancel_max_visible_timeout();
            let delay = device_ai_processing_completion_delay(self.started_at, Instant::now());
            set_device_ai_processing_warning_wait(&self.inner, reason, delay).await;
            self.completed = true;
            self.active = false;
            self.started_at = None;
        }
    }

    fn complete_warning_async(&mut self, reason: &'static str) {
        if device_ai_processing_completion_allowed(self.active, self.completed) {
            self.cancel_max_visible_timeout();
            let delay = device_ai_processing_completion_delay(self.started_at, Instant::now());
            set_device_ai_processing_warning_async(&self.inner, reason, delay);
            self.completed = true;
            self.active = false;
            self.started_at = None;
        }
    }
}

impl Drop for DeviceAiProcessingGuard {
    fn drop(&mut self) {
        if self.active && !self.completed {
            self.cancel_max_visible_timeout();
            set_device_ai_processing_async(&self.inner, false, "dictation_processing_end");
        }
    }
}

fn register_embedded_ble_cancel_flag(inner: &Arc<Inner>, flag: &Arc<AtomicBool>) {
    *inner.embedded_ble_cancel_flag.lock() = Some(Arc::clone(flag));
}

fn clear_embedded_ble_cancel_flag(inner: &Arc<Inner>, flag: &Arc<AtomicBool>) {
    let mut slot = inner.embedded_ble_cancel_flag.lock();
    if slot
        .as_ref()
        .is_some_and(|current| Arc::ptr_eq(current, flag))
    {
        *slot = None;
    }
}

fn request_embedded_ble_capture_cancel_flag(inner: &Arc<Inner>) -> bool {
    let flag = inner.embedded_ble_cancel_flag.lock().clone();
    if let Some(flag) = flag {
        flag.store(true, Ordering::SeqCst);
        true
    } else {
        false
    }
}

fn request_embedded_ble_capture_cancel(inner: &Arc<Inner>) -> bool {
    if inner.embedded_ble_cancel_flag.lock().is_none() {
        return false;
    }
    let session_id = inner.state.lock().session_id;
    dispatch_embedded_ble_session_actor_command(
        inner,
        EmbeddedBleSessionActorCommand::CancelCommand,
        Some(session_id),
        "cancel requested for active BLE capture",
        |_| request_embedded_ble_capture_cancel_flag(inner),
    )
}
