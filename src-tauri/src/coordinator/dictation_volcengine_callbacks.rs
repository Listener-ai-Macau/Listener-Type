fn set_volcengine_preview_callbacks(
    asr: &Arc<VolcengineStreamingASR>,
    inner: &Arc<Inner>,
    session_id: SessionId,
) {
    let stop_dispatched = Arc::new(AtomicBool::new(false));
    let stop_completed = Arc::new(AtomicBool::new(false));
    let stop_state = Arc::new(Mutex::new(EndpointStopDispatchState::default()));
    let endpoint_clock = Arc::new(Mutex::new(SettledTargetEndpointClock::default()));
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

    let inner_for_stream = Arc::clone(inner);
    let clock_for_stream = Arc::clone(&endpoint_clock);
    let asr_for_stream = Arc::clone(asr);
    asr.set_partial_transcript_callback(Some(Arc::new(move |text| {
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

    let inner_for_visual_stream = Arc::clone(inner);
    asr.set_visual_partial_transcript_callback(Some(Arc::new(move |text| {
        update_embedded_audio_visual_preview(&inner_for_visual_stream, session_id, text);
    })));

    let inner_for_partial = Arc::clone(inner);
    let clock_for_partial = Arc::clone(&endpoint_clock);
    let asr_for_partial = Arc::clone(asr);
    asr.set_final_intermediate_transcript_callback(Some(Arc::new(move |update| {
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

    let inner_for_speaker = Arc::clone(inner);
    let clock_for_speaker = Arc::clone(&endpoint_clock);
    let asr_for_speaker = Arc::clone(asr);
    let stop_dispatched_for_speaker = Arc::clone(&stop_dispatched);
    let stop_completed_for_speaker = Arc::clone(&stop_completed);
    let stop_state_for_speaker = Arc::clone(&stop_state);
    asr.set_target_speaker_update_callback(Some(Arc::new(move |update| {
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
        let local_vad_evidence = use_manual_vad
            .then(|| asr_for_speaker.local_speech_activity_snapshot());
        let local_vad_revision = local_vad_evidence.map(|evidence| evidence.revision);
        let decision_update = if use_manual_vad {
            asr_for_speaker.endpoint_update_with_local_speech_evidence_snapshot(
                update,
                local_vad_evidence.expect("manual endpoint VAD evidence snapshot"),
            )
        } else {
            update
        };
        let now = Instant::now();
        {
            let mut clock = clock_for_speaker.lock();
            if let Some(evidence) = local_vad_evidence {
                clock.note_local_vad_evidence(evidence);
            }
            clock.observe_with_local_vad_revision(
                &decision_update,
                endpoint_policy.body_started,
                now,
                local_vad_revision,
            );
        }
        renew_firmware_lease_from_owner_observation(
            &inner_for_speaker,
            session_id,
            &clock_for_speaker,
            &decision_update,
            endpoint_policy.body_started,
        );
        // Provider callbacks only publish observations. Reuse the same clock
        // decision as the watchdog instead of running a second endpoint policy
        // here; the previous split could commit Stopping in the clock and then
        // discard it during a second callback-side evaluation.
        let owner_analysis_pending = asr_for_speaker.local_speaker_analysis_pending();
        let committed_update = clock_for_speaker
            .lock()
            .reduce_session_policy_with_local_vad_revision(
                Instant::now(),
                endpoint_policy,
                &decision_update,
                owner_analysis_pending,
                local_vad_revision,
            );
        if let Some(committed_update) = committed_update {
            handle_target_speaker_endpoint_stop(
                &inner_for_speaker,
                session_id,
                &stop_dispatched_for_speaker,
                &stop_completed_for_speaker,
                &stop_state_for_speaker,
                &clock_for_speaker,
                &asr_for_speaker,
                committed_update,
                endpoint_policy,
            );
        }
    })));
}
