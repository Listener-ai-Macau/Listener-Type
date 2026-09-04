pub(super) fn commit_recording_stop(
    inner: &Arc<Inner>,
    session_id: SessionId,
    source: &'static str,
) -> bool {
    let committed = inner
        .recording_lifecycle
        .lock()
        .commit_stop(session_id);
    if !committed {
        log::info!(
            "[asr] recording lifecycle rejected stale/duplicate automatic stop session_id={session_id} source={source}"
        );
    }
    committed
}

pub(super) fn reopen_recording_stop(inner: &Arc<Inner>, session_id: SessionId) {
    let _ = inner
        .recording_lifecycle
        .lock()
        .reopen_after_failed_stop(session_id);
}

/// Reduce every fresh identity/provider observation into the product
/// lifecycle. This must run when evidence arrives, not only after the endpoint
/// clock is already due: doing the latter left the public lifecycle stuck in
/// QuietPending throughout active owner speech and delayed firmware lease
/// renewal until the stop path.
fn renew_firmware_lease_from_owner_observation(
    inner: &Arc<Inner>,
    session_id: SessionId,
    endpoint_clock: &Arc<Mutex<SettledTargetEndpointClock>>,
    update: &crate::asr::volcengine::TargetSpeakerUpdate,
    body_started: bool,
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
    let attributed_owner_activity = (update.target_activity_advanced
        || update.pending_activity_advanced)
        && target_speaker_update_has_live_owner_activity(update);
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
}

fn handle_target_speaker_endpoint_stop(
    inner: &Arc<Inner>,
    session_id: SessionId,
    stop_dispatched: &Arc<AtomicBool>,
    endpoint_clock: &Arc<Mutex<SettledTargetEndpointClock>>,
    update: crate::asr::volcengine::TargetSpeakerUpdate,
    endpoint_policy: TargetSpeakerEndpointPolicy,
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
    // The callback/watchdog already resolved the product session mode before
    // committing the endpoint decision. Never recalculate it here: doing so
    // previously let the controller stop on a 900 ms body clock and then label
    // that same decision as a 3000 ms no-body stop.
    let body_started = endpoint_policy.body_started;
    let fusion_state = target_speaker_fusion_state(&update);
    let endpoint_timeout_ms = endpoint_policy.endpoint_timeout_ms;
    let stop_reason = endpoint_policy.stop_reason;
    let provider_stall_confirmed =
        provider_progress_stalled(inner, session_id, &update, Instant::now());
    // The endpoint clock is the sole evidence authority. Provider callbacks
    // and the watchdog pass the same reducer snapshot after it has proposed a
    // stop. Re-evaluating a second policy here used to discard that proposal
    // and leave the session in Listening forever.
    // Claim this endpoint proposal before entering the shared STOP transaction.
    // Provider and watchdog callbacks can arrive concurrently; letting each one
    // touch the lifecycle produced misleading duplicate-stop rejections even
    // though only one physical write was sent.
    if stop_dispatched
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
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
        "[asr] owner endpoint claimed stop proposal session_id={session_id} lifecycle={endpoint_lifecycle:?} reason={stop_reason} body_started={} endpoint_timeout_ms={} wall_clock_timeout_ms={} initial_body_wait_active={}",
        endpoint_policy.body_started,
        endpoint_policy.endpoint_timeout_ms,
        endpoint_policy.wall_clock_timeout_ms,
        endpoint_policy.initial_body_wait_active,
    );

    let inner = Arc::clone(inner);
    let stop_dispatched = Arc::clone(stop_dispatched);
    let endpoint_clock = Arc::clone(endpoint_clock);
    let early_final_asr = clone_volcengine_asr_for_session(&inner, session_id);
    async_runtime::spawn(async move {
        let stop_result =
            request_embedded_ble_recording_stop_from_host(&inner, stop_reason).await;
        match stop_result {
            Ok(true) => {
                // Only cross the public Listening -> Processing boundary after
                // the physical STOP write succeeds. The previous eager UI
                // transition plus concurrent final-frame send made a transient
                // BLE error unrecoverable: lifecycle reopened, but coordinator
                // state and ASR were already terminal.
                let stop_feedback_started = Instant::now();
                let feedback_emitted =
                    request_embedded_audio_stop_feedback(&inner, stop_reason);
                maybe_start_polish_prefetch(&inner, session_id);
                if feedback_emitted {
                    log::info!(
                        "[asr] stop_to_transcribing_ms={} session_id={session_id} reason={stop_reason} timeout_ms={endpoint_timeout_ms} body_started={body_started} sentence_pause={} semantic_continuation={} fusion_state={fusion_state:?}",
                        stop_feedback_started.elapsed().as_millis(),
                        preview_ends_with_sentence_terminal(preview.as_deref()),
                        preview_has_dangling_continuation(preview.as_deref()),
                    );
                }
                if let Some(asr) = early_final_asr {
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
                }
                log::info!(
                    "[embedded-ble] target-speaker auto-stop sent session_id={session_id} reason={stop_reason}"
                );
                // If the pre-activation physical segment already ended and no
                // replacement arrived, there will never be another device
                // STOP to drive the product pipeline. Finalize the logical
                // wake-only session now. A replacement segment clears this
                // actor hand-off before we inspect it, so active body capture
                // retains the ordinary physical completion path.
                if !body_started
                    && take_embedded_ble_awaiting_post_activation_segment(&inner, session_id)
                {
                    log::info!(
                        "[embedded-ble] logical no-body endpoint owns finalization after pre-activation segment rotation session_id={session_id}"
                    );
                    if let Err(err) = end_embedded_ble_session(
                        &inner,
                        false,
                        format!(
                            "logical_no_body_after_rotated_segment session_id={session_id} reason={stop_reason}"
                        ),
                    )
                    .await
                    {
                        log::warn!(
                            "[embedded-ble] logical no-body finalization failed session_id={session_id}: {err}"
                        );
                    }
                }
            }
            Ok(false) => {
                // A transient Starting/Listening ownership race must not burn
                // the one-shot endpoint latch forever. A later provider/local
                // update may retry while the same session is still active.
                stop_dispatched.store(false, Ordering::SeqCst);
                endpoint_clock.lock().reopen_after_failed_stop();
                log::info!(
                    "[embedded-ble] target-speaker auto-stop not dispatched; retry armed session_id={session_id} reason={stop_reason}"
                );
            }
            Err(err) => {
                stop_dispatched.store(false, Ordering::SeqCst);
                endpoint_clock.lock().reopen_after_failed_stop();
                log::warn!(
                    "[embedded-ble] target-speaker auto-stop failed; retry armed session_id={session_id} reason={stop_reason}: {err}"
                );
            }
        }
    });
}
