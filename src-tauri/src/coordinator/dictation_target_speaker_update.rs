fn handle_target_speaker_update(
    inner: &Arc<Inner>,
    session_id: SessionId,
    stop_dispatched: &Arc<AtomicBool>,
    endpoint_clock: &Arc<Mutex<SettledTargetEndpointClock>>,
    update: crate::asr::volcengine::TargetSpeakerUpdate,
    endpoint_decision_committed: bool,
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
    let preview = current_embedded_audio_partial_preview(inner);
    // Prefer the live filtered preview if present; wake guard body_started is
    // the durable latch once any non-empty body was seen this session.
    let body_started = automatic_wake_body_started(inner, session_id)
        || preview
            .as_deref()
            .is_some_and(|text| !text.trim().is_empty());
    let attributed_owner_activity = (update.target_activity_advanced
        || update.pending_activity_advanced)
        && target_speaker_update_has_live_owner_activity(&update);
    // Evaluate even when attributed activity already renews the firmware so
    // the same local edge cannot renew a second time on a repeated callback.
    let owner_catch_up_lease_due = endpoint_clock
        .lock()
        .should_renew_firmware_endpoint_lease(&update, body_started);
    let renew_owner_catch_up_lease = !attributed_owner_activity && owner_catch_up_lease_due;
    if attributed_owner_activity || renew_owner_catch_up_lease {
        if renew_owner_catch_up_lease {
            log::info!(
                "[asr] bounded owner catch-up renewed firmware endpoint session_id={session_id} provider_audio_ms={:?} local_audio_ms={:?} local_speech_end_ms={:?} cloud_target_end_ms={:?} local_target_end_ms={:?}",
                update.provider_audio_duration_ms,
                update.audio_duration_ms,
                update.local_speech_end_ms,
                update.target_speech_end_ms,
                update.local_target_speech_end_ms,
            );
        }
        note_embedded_asr_speech_activity(inner, session_id);
        let _ = inner
            .recording_lifecycle
            .lock()
            .note_owner_activity(session_id);
    } else if update.target_activity_advanced || update.pending_activity_advanced {
        log::info!(
            "[asr] stale attributed activity did not refresh firmware endpoint provider_audio_ms={:?} local_audio_ms={:?} cloud_target_end_ms={:?} local_target_end_ms={:?} stable_attributed_end_ms={:?}",
            update.provider_audio_duration_ms,
            update.audio_duration_ms,
            update.target_speech_end_ms,
            update.local_target_speech_end_ms,
            update.stable_attributed_speech_end_ms,
        );
    }
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
    let provider_stall_confirmed =
        provider_progress_stalled(inner, session_id, &update, Instant::now());
    // The endpoint clock is the sole stop authority. Both provider callbacks
    // and the watchdog pass only a snapshot for which the controller has
    // already committed OwnerActive -> Stopping. Re-evaluating a second
    // policy here used to discard that decision and leave the session in
    // Listening forever.
    let endpoint_due = endpoint_decision_committed;
    if !endpoint_due
        || stop_dispatched
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
    {
        return;
    }

    // The endpoint clock has produced evidence, but the product lifecycle is
    // the only component allowed to cross the irreversible stop boundary.
    // Untracked non-embedded sessions retain the legacy host endpoint path;
    // embedded sessions must commit here exactly once.
    let lifecycle_stop_committed = {
        let mut lifecycle = inner.recording_lifecycle.lock();
        match lifecycle.state() {
            crate::speech_decision_kernel::RecordingLifecycleState::Idle
                => true,
            _ => lifecycle.commit_stop(session_id),
        }
    };
    if !lifecycle_stop_committed {
        log::info!(
            "[asr] recording lifecycle rejected stale/duplicate endpoint stop session_id={session_id}"
        );
        stop_dispatched.store(false, Ordering::SeqCst);
        return;
    }

    let provider_stall_fallback =
        provider_stall_local_endpoint_due(&update, provider_stall_confirmed, endpoint_timeout_ms);
    if provider_stall_fallback {
        log::info!(
            "[asr] target endpoint using bounded provider-stall fallback provider_audio_ms={:?} local_audio_ms={:?} cloud_target_end_ms={:?} local_target_end_ms={:?} timeout_ms={endpoint_timeout_ms}",
            update.provider_audio_duration_ms,
            update.audio_duration_ms,
            update.target_speech_end_ms,
            update.local_target_speech_end_ms
        );
    }

    let endpoint_lifecycle = endpoint_clock.lock().lifecycle();
    log::info!(
        "[asr] owner endpoint committed stop session_id={session_id} lifecycle={endpoint_lifecycle:?} reason={stop_reason}"
    );

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
    let endpoint_clock = Arc::clone(endpoint_clock);
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
                    "[asr] proactive endpoint final frame sent session_id={session_id} provider_stall_fallback={provider_stall_fallback} controller_committed=true elapsed_ms={}",
                    started.elapsed().as_millis()
                ),
                Err(err) => log::warn!(
                    "[asr] proactive endpoint final frame failed session_id={session_id} provider_stall_fallback={provider_stall_fallback} controller_committed=true elapsed_ms={} error={err}",
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
                endpoint_clock.lock().reopen_after_failed_stop();
                let _ = inner
                    .recording_lifecycle
                    .lock()
                    .reopen_after_failed_stop(session_id);
                log::info!(
                    "[embedded-ble] target-speaker auto-stop not dispatched; retry armed session_id={session_id} reason={stop_reason}"
                );
            }
            Err(err) => {
                stop_dispatched.store(false, Ordering::SeqCst);
                endpoint_clock.lock().reopen_after_failed_stop();
                let _ = inner
                    .recording_lifecycle
                    .lock()
                    .reopen_after_failed_stop(session_id);
                log::warn!(
                    "[embedded-ble] target-speaker auto-stop failed; retry armed session_id={session_id} reason={stop_reason}: {err}"
                );
            }
        }
    });
}
