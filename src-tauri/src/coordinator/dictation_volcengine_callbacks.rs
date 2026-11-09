fn set_volcengine_preview_callbacks(
    asr: &Arc<VolcengineStreamingASR>,
    inner: &Arc<Inner>,
    session_id: SessionId,
) {
    let stop_dispatched = Arc::new(AtomicBool::new(false));
    let stop_completed = Arc::new(AtomicBool::new(false));
    let stop_state = Arc::new(Mutex::new(EndpointStopDispatchState::default()));
    let endpoint_clock = Arc::new(Mutex::new(SettledTargetEndpointClock::default()));
    #[cfg(target_os = "windows")]
    { endpoint_clock.lock().native_capture_endpoint = native_microphone_endpoint_owns(inner, session_id); }
    #[cfg(test)]
    tests::track_question_endpoint_clock(&endpoint_clock);
    log::info!(
        "[asr] endpoint callbacks installed session_id={session_id} asr_instance=0x{:x}",
        Arc::as_ptr(asr) as usize,
    );
    start_settled_target_endpoint_watchdog(
        inner,
        session_id,
        &stop_dispatched,
        &stop_completed,
        &stop_state,
        &endpoint_clock,
        asr,
    );

    let inner_for_stream = Arc::downgrade(inner);
    let clock_for_stream = Arc::clone(&endpoint_clock);
    let asr_for_stream = Arc::downgrade(asr);
    asr.set_partial_transcript_callback(Some(Arc::new(move |text| {
        let (Some(inner_for_stream), Some(asr_for_stream)) =
            (inner_for_stream.upgrade(), asr_for_stream.upgrade()) else { return; };
        note_automatic_wake_transcript_timing(
            &inner_for_stream, session_id, asr_for_stream.provider_timed_transcript_snapshot(),
        );
        note_automatic_wake_transcript_timing(
            &inner_for_stream, session_id, asr_for_stream.transcript_timing_snapshot_for(&text),
        );
        let previous_preview = current_embedded_audio_partial_preview(&inner_for_stream);
        let preview_changed =
            update_embedded_audio_partial_preview(&inner_for_stream, session_id, text);
        let current_preview = current_embedded_audio_partial_preview(&inner_for_stream);
        let raw_endpoint_snapshot = asr_for_stream.endpoint_update_snapshot();
        let endpoint_policy = resolve_target_speaker_endpoint_policy(
            &inner_for_stream,
            session_id,
            current_preview.as_deref(),
            raw_endpoint_snapshot
                .audio_duration_ms
                .or(raw_endpoint_snapshot.provider_audio_duration_ms),
        );
        let endpoint_snapshot = if manual_endpoint_vad_allowed(
            &inner_for_stream,
            session_id,
            endpoint_policy,
            &raw_endpoint_snapshot,
        ) {
            let evidence = asr_for_stream.local_speech_activity_snapshot();
            asr_for_stream.endpoint_update_with_local_speech_evidence_snapshot(
                raw_endpoint_snapshot,
                evidence,
            )
        } else {
            raw_endpoint_snapshot
        };
        if !endpoint_policy.body_started {
            return;
        }
        // Volcengine may publish visible body text before its first target
        // speaker row. Seed the same controller with the ASR state snapshot so
        // the owner endpoint exists even on a delayed diarization path.
        clock_for_stream.lock().seed_from_snapshot_if_missing(
            endpoint_snapshot,
            endpoint_policy.body_started,
            Instant::now(),
        );
        let refresh_firmware_speech = preview_changed
            && clock_for_stream.lock().latest_update.as_ref().is_some_and(|update| {
                authoritative_preview_growth_has_recent_owner_speech(
                    update,
                    previous_preview.as_deref(),
                    current_preview.as_deref(),
                )
            });
        arm_settled_target_endpoint_for_visible_body(
            &inner_for_stream,
            session_id,
            &clock_for_stream,
            endpoint_policy.body_started,
        );
        if refresh_firmware_speech {
            log::info!(
                "[asr] authoritative preview growth refreshed firmware speech protection session_id={session_id} previous_chars={} current_chars={}",
                previous_preview.as_deref().map_or(0, |text| text.chars().count()),
                current_preview.as_deref().map_or(0, |text| text.chars().count()),
            );
            note_embedded_asr_speech_activity(&inner_for_stream, session_id);
        }
    })));

    let inner_for_visual_stream = Arc::downgrade(inner);
    let asr_for_visual_stream = Arc::downgrade(asr);
    asr.set_visual_partial_transcript_callback(Some(Arc::new(move |update| {
        let (Some(inner_for_visual_stream), Some(asr_for_visual_stream)) =
            (inner_for_visual_stream.upgrade(), asr_for_visual_stream.upgrade()) else { return; };
        let text = update.text().to_string();
        note_automatic_wake_transcript_timing(
            &inner_for_visual_stream, session_id, asr_for_visual_stream.provider_timed_transcript_snapshot(),
        );
        note_automatic_wake_transcript_timing(
            &inner_for_visual_stream, session_id, asr_for_visual_stream.transcript_timing_snapshot_for(&text),
        );
        match update {
            crate::asr::volcengine::VisualTranscriptUpdate::Provisional(_) => {
                update_embedded_audio_visual_preview(&inner_for_visual_stream, session_id, text);
            }
            crate::asr::volcengine::VisualTranscriptUpdate::ConfirmedOwnerPartition(_) => {
                reconcile_embedded_audio_owner_visual(&inner_for_visual_stream, session_id, text);
            }
        }
    })));

    let inner_for_partial = Arc::downgrade(inner);
    let clock_for_partial = Arc::clone(&endpoint_clock);
    let asr_for_partial = Arc::downgrade(asr);
    asr.set_final_intermediate_transcript_callback(Some(Arc::new(move |update| {
        let (Some(inner_for_partial), Some(asr_for_partial)) =
            (inner_for_partial.upgrade(), asr_for_partial.upgrade()) else { return; };
        note_automatic_wake_transcript_timing(
            &inner_for_partial, session_id, asr_for_partial.provider_timed_transcript_snapshot(),
        );
        note_automatic_wake_transcript_timing(
            &inner_for_partial, session_id, asr_for_partial.transcript_timing_snapshot_for(&update.text),
        );
        let previous_preview = current_embedded_audio_partial_preview(&inner_for_partial);
        let preview_changed = update_embedded_audio_partial_preview_from_final_supplement(
            &inner_for_partial,
            session_id,
            update,
        );
        let current_preview = current_embedded_audio_partial_preview(&inner_for_partial);
        let raw_endpoint_snapshot = asr_for_partial.endpoint_update_snapshot();
        let endpoint_policy = resolve_target_speaker_endpoint_policy(
            &inner_for_partial,
            session_id,
            current_preview.as_deref(),
            raw_endpoint_snapshot
                .audio_duration_ms
                .or(raw_endpoint_snapshot.provider_audio_duration_ms),
        );
        let endpoint_snapshot = if manual_endpoint_vad_allowed(
            &inner_for_partial,
            session_id,
            endpoint_policy,
            &raw_endpoint_snapshot,
        ) {
            let evidence = asr_for_partial.local_speech_activity_snapshot();
            asr_for_partial.endpoint_update_with_local_speech_evidence_snapshot(
                raw_endpoint_snapshot,
                evidence,
            )
        } else {
            raw_endpoint_snapshot
        };
        if !endpoint_policy.body_started {
            return;
        }
        clock_for_partial.lock().seed_from_snapshot_if_missing(
            endpoint_snapshot,
            endpoint_policy.body_started,
            Instant::now(),
        );
        let refresh_firmware_speech = preview_changed
            && clock_for_partial.lock().latest_update.as_ref().is_some_and(|update| {
                authoritative_preview_growth_has_recent_owner_speech(
                    update,
                    previous_preview.as_deref(),
                    current_preview.as_deref(),
                )
            });
        arm_settled_target_endpoint_for_visible_body(
            &inner_for_partial,
            session_id,
            &clock_for_partial,
            endpoint_policy.body_started,
        );
        if refresh_firmware_speech {
            log::info!(
                "[asr] authoritative final supplement growth refreshed firmware speech protection session_id={session_id} previous_chars={} current_chars={}",
                previous_preview.as_deref().map_or(0, |text| text.chars().count()),
                current_preview.as_deref().map_or(0, |text| text.chars().count()),
            );
            note_embedded_asr_speech_activity(&inner_for_partial, session_id);
        }
    })));

    let inner_for_speaker = Arc::downgrade(inner);
    let clock_for_speaker = Arc::clone(&endpoint_clock);
    let asr_for_speaker = Arc::downgrade(asr);
    asr.set_target_speaker_update_callback(Some(Arc::new(move |update| {
        let (Some(inner_for_speaker), Some(asr_for_speaker)) =
            (inner_for_speaker.upgrade(), asr_for_speaker.upgrade()) else { return; };
        let preview = current_embedded_audio_endpoint_preview(&inner_for_speaker);
        let decision_audio_ms = update.audio_duration_ms.or(update.provider_audio_duration_ms);
        let endpoint_policy = resolve_target_speaker_endpoint_policy(
            &inner_for_speaker,
            session_id,
            preview.as_deref(),
            decision_audio_ms,
        );
        let use_manual_vad = manual_endpoint_vad_allowed(
            &inner_for_speaker,
            session_id,
            endpoint_policy,
            &update,
        );
        // Automatic owner updates also need the current PCM/VAD epoch at the
        // moment a qualified edge arrives. It is a STOP veto, not an owner
        // classification or a replacement for the automatic decision update.
        let local_vad_evidence = asr_for_speaker.local_speech_activity_snapshot();
        let local_vad_revision = use_manual_vad.then_some(local_vad_evidence.revision);
        let decision_update = if use_manual_vad {
            asr_for_speaker.endpoint_update_with_local_speech_evidence_snapshot(
                update,
                local_vad_evidence,
            )
        } else {
            update
        };
        let now = Instant::now();
        let personal_owner_advanced = {
            let mut clock = clock_for_speaker.lock();
            let personal_observation = asr_for_speaker.personal_vad_snapshot();
            let personal_owner_advanced = asr_for_speaker.personal_vad_started()
            && clock.note_personal_presence(personal_observation, Instant::now());
            clock.note_local_vad_evidence_at(local_vad_evidence, now);
            clock.observe_with_local_vad_revision(
                &decision_update,
                endpoint_policy.body_started,
                now,
                local_vad_revision,
            );
            personal_owner_advanced
        };
        if personal_owner_advanced { note_embedded_asr_speech_activity(&inner_for_speaker, session_id); }
        renew_firmware_lease_from_owner_observation(
            &inner_for_speaker,
            session_id,
            &clock_for_speaker,
            &decision_update,
            endpoint_policy.body_started,
        );
        // Publish observations only. The watchdog first offers ready body
        // text to the ordinary streaming transport, then makes the one STOP
        // decision from fresh observations. Dispatching here could preempt
        // that final ordinary tick depending on provider callback order.
    })));
}

#[cfg(test)]
mod callback_resource_lifetime_tests {
    use super::*;

    #[tokio::test]
    async fn recognition_callbacks_and_watchdog_release_the_finished_session_and_parent() {
        for (phase, cancelled, shutting_down) in [
            (SessionPhase::Idle, false, false),
            (SessionPhase::Idle, true, false),
            (SessionPhase::Listening, false, true),
        ] {
            let coordinator = crate::coordinator::Coordinator::new();
            let session_id = new_session_id();
            {
                let mut state = coordinator.inner.state.lock();
                state.session_id = session_id;
                state.phase = phase;
                state.cancelled = cancelled;
            }
            coordinator.inner.shutdown.store(shutting_down, Ordering::SeqCst);
            let asr = Arc::new(VolcengineStreamingASR::new(crate::asr::VolcengineCredentials {
                app_id: "offline-lifetime-test".into(), access_token: "not-a-credential".into(),
                resource_id: crate::asr::VolcengineCredentials::default_resource_id().into(),
            }, Vec::new()));
            let weak_asr = Arc::downgrade(&asr);
            let weak_parent = Arc::downgrade(&coordinator.inner);
            // Register all real callbacks and the real watchdog. No provider
            // connection, model, capture, editor event or device is started.
            set_volcengine_preview_callbacks(&asr, &coordinator.inner, session_id);
            if cancelled { asr.cancel(); }
            drop(asr);
            let deadline = Instant::now() + Duration::from_millis(600);
            while weak_asr.upgrade().is_some() && Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert!(weak_asr.upgrade().is_none(),
                "ASR owns its callbacks, so callbacks/watchdog cannot own ASR: phase={phase:?} cancelled={cancelled} shutdown={shutting_down}");
            drop(coordinator);
            tokio::time::sleep(Duration::from_millis(60)).await;
            assert!(weak_parent.upgrade().is_none(), "session observers cannot keep the coordinator alive");
        }
    }
}
