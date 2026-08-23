fn handle_target_speaker_update(
    inner: &Arc<Inner>,
    session_id: SessionId,
    stop_dispatched: &Arc<AtomicBool>,
    update: crate::asr::volcengine::TargetSpeakerUpdate,
    settled_wall_clock_due: bool,
) {
    let session_active = {
        let state = inner.state.lock();
        state.session_id == session_id
            && !state.cancelled
            && matches!(
                state.phase,
                SessionPhase::Starting | SessionPhase::Listening
            )
    };
    if !session_active {
        return;
    }
    if update.target_activity_advanced || update.pending_activity_advanced {
        if target_speaker_update_has_live_owner_activity(&update) {
            note_embedded_asr_speech_activity(inner, session_id);
        } else {
            log::info!(
                "[asr] stale attributed activity did not refresh firmware endpoint provider_audio_ms={:?} local_audio_ms={:?} cloud_target_end_ms={:?} local_target_end_ms={:?} stable_attributed_end_ms={:?}",
                update.provider_audio_duration_ms,
                update.audio_duration_ms,
                update.target_speech_end_ms,
                update.local_target_speech_end_ms,
                update.stable_attributed_speech_end_ms,
            );
        }
    }
    let preview = current_embedded_audio_partial_preview(inner);
    // Prefer the live filtered preview if present; wake guard body_started is
    // the durable latch once any non-empty body was seen this session.
    let body_started = automatic_wake_body_started(inner, session_id)
        || preview
            .as_deref()
            .is_some_and(|text| !text.trim().is_empty());
    let mode_timeout_ms = target_speaker_end_timeout_ms_for_preview(preview.as_deref());
    let mode_endpoint_timeout_ms = if body_started {
        mode_timeout_ms
    } else if automatic_wake_session_active(inner, session_id) {
        EMBEDDED_AUTOMATIC_WAKE_NO_BODY_END_TIMEOUT_MS.max(mode_timeout_ms)
    } else {
        mode_timeout_ms
    };
    let fusion_state = target_speaker_fusion_state(&update);
    let endpoint_timeout_ms =
        target_speaker_endpoint_timeout_with_fusion(fusion_state, mode_endpoint_timeout_ms);
    let stop_reason = target_speaker_inactive_stop_reason(endpoint_timeout_ms);
    let initial_body_wait_active =
        automatic_wake_initial_body_wait_active(inner, session_id, update.audio_duration_ms);
    let provider_stall_confirmed =
        provider_progress_stalled(inner, session_id, &update, Instant::now());
    let provider_clock_endpoint_due = target_speaker_endpoint_due_with_provider_stall(
        &update,
        provider_stall_confirmed,
        endpoint_timeout_ms,
    );
    let settled_wall_clock_endpoint_due = body_started && settled_wall_clock_due;
    let endpoint_due = !initial_body_wait_active
        && target_speaker_endpoint_due_after_visible_body_gate(
            body_started,
            provider_clock_endpoint_due,
            settled_wall_clock_endpoint_due,
        );
    if !endpoint_due
        || stop_dispatched
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
    {
        return;
    }

    let provider_stall_fallback =
        provider_stall_local_endpoint_due(&update, provider_stall_confirmed, endpoint_timeout_ms);
    if settled_wall_clock_endpoint_due && !provider_clock_endpoint_due {
        log::info!(
            "[asr] target endpoint using settled-text wall clock provider_audio_ms={:?} local_audio_ms={:?} cloud_target_end_ms={:?} local_target_end_ms={:?} timeout_ms={endpoint_timeout_ms}",
            update.provider_audio_duration_ms,
            update.audio_duration_ms,
            update.target_speech_end_ms,
            update.local_target_speech_end_ms
        );
    }
    if provider_stall_fallback {
        log::info!(
            "[asr] target endpoint using bounded provider-stall fallback provider_audio_ms={:?} local_audio_ms={:?} cloud_target_end_ms={:?} local_target_end_ms={:?} timeout_ms={endpoint_timeout_ms}",
            update.provider_audio_duration_ms,
            update.audio_duration_ms,
            update.target_speech_end_ms,
            update.local_target_speech_end_ms
        );
    }

    // 1.0.4 A3: latch Transcribing + keep last preview immediately at the
    // silence threshold so the UI does not hang on Listening while BLE stop
    // and final-frame work are still in flight.
    let stop_feedback_started = Instant::now();
    let feedback_emitted = request_embedded_audio_stop_feedback(inner, stop_reason);
    // 预热润色：与 stop/终稿并行发起，终稿一致则采用，首字提前 ~0.4-0.6s。
    maybe_start_polish_prefetch(inner, session_id);
    if feedback_emitted {
        log::info!(
            "[asr] stop_to_transcribing_ms={} session_id={session_id} reason={stop_reason} timeout_ms={endpoint_timeout_ms} body_started={body_started} sentence_pause={} semantic_continuation={} fusion_state={fusion_state:?}",
            stop_feedback_started.elapsed().as_millis(),
            preview_ends_with_sentence_terminal(preview.as_deref()),
            preview_has_dangling_continuation(preview.as_deref()),
        );
    }

    let inner = Arc::clone(inner);
    let stop_dispatched = Arc::clone(stop_dispatched);
    let early_final_asr = clone_volcengine_asr_for_session(&inner, session_id);
    async_runtime::spawn(async move {
        let stop_future = request_embedded_ble_recording_stop_from_host(&inner, stop_reason);
        let finalization_future = async move {
            let Some(asr) = early_final_asr else {
                return;
            };
            let started = Instant::now();
            match asr.send_last_frame().await {
                Ok(()) => log::info!(
                    "[asr] proactive endpoint final frame sent session_id={session_id} provider_stall_fallback={provider_stall_fallback} settled_wall_clock_fallback={settled_wall_clock_endpoint_due} elapsed_ms={}",
                    started.elapsed().as_millis()
                ),
                Err(err) => log::warn!(
                    "[asr] proactive endpoint final frame failed session_id={session_id} provider_stall_fallback={provider_stall_fallback} settled_wall_clock_fallback={settled_wall_clock_endpoint_due} elapsed_ms={} error={err}",
                    started.elapsed().as_millis()
                ),
            }
        };
        let (stop_result, ()) = tokio::join!(stop_future, finalization_future);
        match stop_result {
            Ok(true) => log::info!(
                "[embedded-ble] target-speaker auto-stop sent session_id={session_id} reason={stop_reason}"
            ),
            Ok(false) => {
                // A transient Starting/Listening ownership race must not burn
                // the one-shot endpoint latch forever. A later provider/local
                // update may retry while the same session is still active.
                stop_dispatched.store(false, Ordering::SeqCst);
                log::info!(
                    "[embedded-ble] target-speaker auto-stop not dispatched; retry armed session_id={session_id} reason={stop_reason}"
                );
            }
            Err(err) => {
                stop_dispatched.store(false, Ordering::SeqCst);
                log::warn!(
                    "[embedded-ble] target-speaker auto-stop failed; retry armed session_id={session_id} reason={stop_reason}: {err}"
                );
            }
        }
    });
}

