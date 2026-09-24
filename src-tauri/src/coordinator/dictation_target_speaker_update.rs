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

fn commit_recording_stop_owned(
    inner: &Arc<Inner>,
    session_id: SessionId,
    reason: &'static str,
    attempt_id: u64,
) -> bool {
    let committed = inner
        .recording_lifecycle
        .lock()
        .commit_stop_owned(session_id, Some(attempt_id));
    if !committed {
        log::info!(
            "[asr] recording lifecycle rejected stale/duplicate automatic owned stop session_id={session_id} attempt_id={attempt_id} source={reason}"
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

fn reopen_recording_stop_owned(inner: &Arc<Inner>, session_id: SessionId, attempt_id: u64) -> bool {
    inner
        .recording_lifecycle
        .lock()
        .reopen_after_failed_stop_owned(session_id, Some(attempt_id))
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
    let qualified_owner_activity = update.qualified_owner_activity_advanced
        && has_current_qualified_owner_observation(update);
    let attributed_owner_activity = qualified_owner_activity
        || ((update.target_activity_advanced || update.pending_activity_advanced)
            && target_speaker_update_has_live_owner_activity(update));
    // Evaluate even when attributed activity already renews the firmware so
    // the same local edge cannot renew a second time on a repeated callback.
    let owner_catch_up_lease_due = endpoint_clock
        .lock()
        .should_renew_firmware_endpoint_lease(&update, body_started);
    let renew_owner_catch_up_lease = !attributed_owner_activity && owner_catch_up_lease_due;
    if attributed_owner_activity || renew_owner_catch_up_lease {
        if renew_owner_catch_up_lease {
            log::info!(
                "[asr] bounded owner catch-up renewed firmware endpoint session_id={session_id} provider_audio_ms={:?} local_audio_ms={:?} local_speech_end_ms={:?} qualified_owner_end_ms={:?} qualified_advanced={} cloud_target_end_ms={:?} local_target_end_ms={:?}",
                update.provider_audio_duration_ms,
                update.audio_duration_ms,
                update.local_speech_end_ms,
                update.qualified_owner_speech_end_ms,
                update.qualified_owner_activity_advanced,
                update.target_speech_end_ms,
                update.local_target_speech_end_ms,
            );
        }
        note_embedded_asr_speech_activity(inner, session_id);
    } else if update.target_activity_advanced
        || update.pending_activity_advanced
        || update.qualified_owner_activity_advanced
    {
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

const ENDPOINT_STOP_MAX_LOCAL_VAD_LAG_MS: u64 = 250;
// The watchdog's wall clock only schedules a candidate.  Before the physical
// STOP write, the local VAD must also have analysed a full endpoint interval
// after the last confirmed speech sample.  Without this audio-time gate, a
// worker that is 30 ms behind can authorize STOP just before it publishes a
// new PendingSpeech onset (the r10 failure).
const ENDPOINT_STOP_MIN_CONFIRMED_SILENCE_MS: u64 = EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS;

fn local_vad_confirmed_silence_ms(
    evidence: &crate::asr::volcengine::LocalSpeechEvidence,
) -> Option<u64> {
    evidence
        .last_detected_speech_end_ms
        .map(|speech_end_ms| evidence.analyzed_through_ms.saturating_sub(speech_end_ms))
}

fn local_vad_stop_silence_is_qualified(
    evidence: &crate::asr::volcengine::LocalSpeechEvidence,
) -> bool {
    local_vad_confirmed_silence_ms(evidence)
        .is_some_and(|silence| silence >= ENDPOINT_STOP_MIN_CONFIRMED_SILENCE_MS)
}

#[derive(Clone)]
struct EndpointStopAdmission {
    asr: Arc<crate::asr::volcengine::VolcengineStreamingASR>,
    endpoint_clock: Arc<Mutex<SettledTargetEndpointClock>>,
    stop_state: Arc<Mutex<EndpointStopDispatchState>>,
    ticket: EndpointStopTicket,
}

impl EndpointStopAdmission {
    fn is_valid(&self, inner: &Arc<Inner>) -> bool {
        if !self.stop_state.lock().is_current(self.ticket) {
            return false;
        }
        let session_active = {
            let state = inner.state.lock();
            state.session_id == self.ticket.session_id
                && !state.cancelled
                && matches!(
                    state.phase,
                    SessionPhase::Starting | SessionPhase::Listening
                )
        };
        if !session_active {
            return false;
        }
        let endpoint_current = {
            let clock = self.endpoint_clock.lock();
            clock.generation == self.ticket.endpoint_generation && clock.stop_proposed
        };
        if !endpoint_current {
            return false;
        }
        if !self.ticket.manual_vad_guard {
            let automatic_body_with_recent_owner = {
                let clock = self.endpoint_clock.lock();
                clock.automatic_wake_session
                    && !clock.automatic_no_body_armed
                    && clock.positive_owner_evidence_live(Instant::now())
            };
            if automatic_body_with_recent_owner {
                let evidence = self.asr.local_speech_activity_snapshot();
                let update = self.asr.endpoint_update_snapshot();
                if automatic_vad_candidate_holds_stop(evidence, &update, true) {
                    log::info!(
                        "[asr] automatic endpoint STOP admission revoked by live VAD session_id={} proposal_id={} generation={} vad_state={:?} vad_epoch={} analyzed_through_ms={} captured_audio_ms={:?}",
                        self.ticket.session_id,
                        self.ticket.proposal_id,
                        self.ticket.endpoint_generation,
                        evidence.state,
                        evidence.activity_epoch,
                        evidence.analyzed_through_ms,
                        update.audio_duration_ms,
                    );
                    return false;
                }
            }
            return true;
        }

        let evidence = self.asr.local_speech_activity_snapshot();
        let captured_audio_ms = self
            .asr
            .endpoint_update_snapshot()
            .audio_duration_ms
            .or(Some(evidence.analyzed_through_ms));
        let vad_lag_ms = captured_audio_ms
            .map(|captured| captured.saturating_sub(evidence.analyzed_through_ms));
        let confirmed_silence_ms = local_vad_confirmed_silence_ms(&evidence);
        let valid = evidence.state == crate::asr::volcengine::LocalSpeechActivityState::NonSpeech
            && evidence.activity_epoch == self.ticket.activity_epoch
            && evidence.revision >= self.ticket.local_vad_revision
            && vad_lag_ms.is_some_and(|lag| lag <= ENDPOINT_STOP_MAX_LOCAL_VAD_LAG_MS)
            && local_vad_stop_silence_is_qualified(&evidence);
        if !valid {
            log::info!(
                "[asr] endpoint STOP admission revoked session_id={} proposal_id={} generation={} ticket_vad_revision={} current_vad_revision={} ticket_activity_epoch={} current_activity_epoch={} vad_state={:?} analyzed_through_ms={} last_speech_end_ms={:?} confirmed_silence_ms={:?} required_silence_ms={} captured_audio_ms={:?} vad_lag_ms={:?}",
                self.ticket.session_id,
                self.ticket.proposal_id,
                self.ticket.endpoint_generation,
                self.ticket.local_vad_revision,
                evidence.revision,
                self.ticket.activity_epoch,
                evidence.activity_epoch,
                evidence.state,
                evidence.analyzed_through_ms,
                evidence.last_detected_speech_end_ms,
                confirmed_silence_ms,
                ENDPOINT_STOP_MIN_CONFIRMED_SILENCE_MS,
                captured_audio_ms,
                vad_lag_ms,
            );
        }
        valid
    }
}

fn handle_target_speaker_endpoint_stop(
    inner: &Arc<Inner>,
    session_id: SessionId,
    stop_dispatched: &Arc<AtomicBool>,
    stop_completed: &Arc<AtomicBool>,
    stop_state: &Arc<Mutex<EndpointStopDispatchState>>,
    endpoint_clock: &Arc<Mutex<SettledTargetEndpointClock>>,
    asr: &Arc<crate::asr::volcengine::VolcengineStreamingASR>,
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
    let preview = current_embedded_audio_endpoint_preview(inner);
    // The callback/watchdog already resolved the product session mode before
    // committing the endpoint decision. Never recalculate it here: doing so
    // previously let the controller stop on a 900 ms body clock and then label
    // that same decision as a 3000 ms no-body stop.
    let body_started = endpoint_policy.body_started;
    let fusion_state = target_speaker_fusion_state(&update);
    let preview_grew_recently = inner
        .embedded_audio_preview
        .lock()
        .last_visible_growth_at(session_id)
        .is_some_and(|grown_at| {
            grown_at.elapsed()
                < Duration::from_millis(endpoint_policy.endpoint_timeout_ms)
        });
    let body_started_recently = automatic_wake_body_started_recently(
        inner,
        session_id,
        Duration::from_millis(endpoint_policy.endpoint_timeout_ms.max(1_000)),
    );
    if body_started && owner_endpoint_stop_blocked_by_live_owner(
        fusion_state,
        &update,
        body_started,
        preview_grew_recently,
        body_started_recently,
    ) {
        // A concurrent preview can invalidate a proposed stop. Keep the
        // controller retryable after that preview settles.
        endpoint_clock.lock().reopen_after_failed_stop();
        log::info!(
            "[asr] owner still continuing; ignore due endpoint session_id={session_id} fusion_state={fusion_state:?} reason={}",
            endpoint_policy.stop_reason
        );
        return;
    }
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

    let (endpoint_generation, manual_vad_guard) = {
        let clock = endpoint_clock.lock();
        (
            clock.generation,
            manual_endpoint_vad_allowed(inner, session_id, endpoint_policy, &update),
        )
    };
    let vad_evidence = asr.local_speech_activity_snapshot();
    let ticket = {
        let mut state = stop_state.lock();
        state.propose(
            session_id,
            endpoint_generation,
            vad_evidence.revision,
            vad_evidence.activity_epoch,
            manual_vad_guard,
        )
    };
    let Some(ticket) = ticket else {
        stop_dispatched.store(false, Ordering::SeqCst);
        return;
    };
    let endpoint_lifecycle = endpoint_clock.lock().lifecycle();
    log::info!(
        "[asr] owner endpoint proposed STOP session_id={session_id} proposal_id={} generation={} lifecycle={endpoint_lifecycle:?} reason={stop_reason} body_started={} endpoint_timeout_ms={} wall_clock_timeout_ms={} initial_body_wait_active={} manual_vad_guard={}",
        ticket.proposal_id,
        ticket.endpoint_generation,
        endpoint_policy.body_started,
        endpoint_policy.endpoint_timeout_ms,
        endpoint_policy.wall_clock_timeout_ms,
        endpoint_policy.initial_body_wait_active,
        manual_vad_guard,
    );

    let inner = Arc::clone(inner);
    let stop_dispatched = Arc::clone(stop_dispatched);
    let stop_completed = Arc::clone(stop_completed);
    let stop_state = Arc::clone(stop_state);
    let endpoint_clock = Arc::clone(endpoint_clock);
    let admission = EndpointStopAdmission {
        asr: Arc::clone(asr),
        endpoint_clock: Arc::clone(&endpoint_clock),
        stop_state: Arc::clone(&stop_state),
        ticket,
    };
    async_runtime::spawn(async move {
        let stop_result = request_embedded_ble_recording_stop_from_host_for_endpoint(
            &inner,
            stop_reason,
            admission,
        )
        .await;
        match stop_result {
            Ok(true) => {
                stop_state.lock().mark_sent_if_current(ticket);
                stop_completed.store(true, Ordering::SeqCst);
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
                // A successful STOP write only requests capture to stop. The
                // device still drains audio recorded before that boundary.
                // finish_streaming_session flushes the last PCM block and then
                // finalizes ASR when physical completion arrives. Sealing here
                // discarded 1.427 s in installed session 74b7ab15 while its WAV
                // misleadingly retained those bytes. Keep immediate UI feedback
                // above, but leave the provider open for the captured tail.
                log::info!(
                    "[embedded-ble] target-speaker auto-stop sent session_id={session_id} reason={stop_reason}"
                );
                // If the pre-activation physical segment already ended and no
                // replacement arrived, there will never be another device
                // STOP to drive the product pipeline. Finalize the logical
                // wake-only session now. A replacement segment clears this
                // actor hand-off before we inspect it, so active body capture
                // retains the ordinary physical completion path.
                if take_embedded_ble_awaiting_post_activation_segment(&inner, session_id)
                {
                    log::info!(
                        "[embedded-ble] logical no-body endpoint owns finalization after pre-activation segment rotation session_id={session_id}"
                    );
                    let source_integrity_ledger =
                        embedded_source_integrity_ledger_for_session(&inner, session_id);
                    if let Err(err) = end_embedded_ble_session_with_source_integrity(
                        &inner,
                        false,
                        format!(
                            "logical_no_body_after_rotated_segment session_id={session_id} reason={stop_reason}"
                        ),
                        source_integrity_ledger,
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
                let cancelled = stop_state.lock().cancel_if_current(ticket);
                if cancelled {
                    stop_dispatched.store(false, Ordering::SeqCst);
                    endpoint_clock.lock().reopen_after_failed_stop();
                }
                log::info!(
                    "[embedded-ble] target-speaker auto-stop not dispatched; retry armed session_id={session_id} proposal_id={} reason={stop_reason} ticket_current={cancelled}",
                    ticket.proposal_id,
                );
            }
            Err(err) => {
                let failed = stop_state.lock().cancel_if_current(ticket);
                if failed {
                    stop_dispatched.store(false, Ordering::SeqCst);
                    endpoint_clock.lock().reopen_after_failed_stop();
                }
                log::warn!(
                    "[embedded-ble] target-speaker auto-stop failed; retry armed session_id={session_id} proposal_id={} reason={stop_reason} ticket_current={failed}: {err}",
                    ticket.proposal_id,
                );
            }
        }
    });
}
