fn set_volcengine_preview_callbacks(
    asr: &Arc<VolcengineStreamingASR>,
    inner: &Arc<Inner>,
    session_id: SessionId,
) {
    let stop_dispatched = Arc::new(AtomicBool::new(false));
    let endpoint_clock = Arc::new(Mutex::new(SettledTargetEndpointClock::default()));
    start_settled_target_endpoint_watchdog(
        inner,
        session_id,
        &stop_dispatched,
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
        // Volcengine may publish visible body text before its first target
        // speaker row. Seed the same controller with the ASR state snapshot so
        // the owner endpoint exists even on a delayed diarization path.
        clock_for_stream.lock().seed_from_snapshot_if_missing(
            asr_for_stream.endpoint_update_snapshot(),
            !current_preview.as_deref().unwrap_or_default().trim().is_empty(),
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
        clock_for_partial.lock().seed_from_snapshot_if_missing(
            asr_for_partial.endpoint_update_snapshot(),
            !current_preview.as_deref().unwrap_or_default().trim().is_empty(),
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
    asr.set_target_speaker_update_callback(Some(Arc::new(move |update| {
        let preview = current_embedded_audio_partial_preview(&inner_for_speaker);
        let decision_audio_ms = update.audio_duration_ms.or(update.provider_audio_duration_ms);
        let endpoint_policy = resolve_target_speaker_endpoint_policy(
            &inner_for_speaker,
            session_id,
            preview.as_deref(),
            decision_audio_ms,
        );
        let now = Instant::now();
        let endpoint_rearmed = {
            let mut clock = clock_for_speaker.lock();
            clock
                .observe(&update, endpoint_policy.body_started, now)
                .is_some()
        };
        let owner_activity = reduce_target_speaker_activity_observation(
            &inner_for_speaker,
            session_id,
            &clock_for_speaker,
            &update,
            endpoint_policy.body_started,
        );
        if endpoint_rearmed && !owner_activity {
            let _ = inner_for_speaker
                .recording_lifecycle
                .lock()
                .note_quiet_pending(session_id);
        }
        // Provider callbacks only publish observations. Reuse the same clock
        // decision as the watchdog instead of running a second endpoint policy
        // here; the previous split could commit Stopping in the clock and then
        // discard it during a second callback-side evaluation.
        let owner_analysis_pending = asr_for_speaker.local_speaker_analysis_pending();
        let committed_update = clock_for_speaker.lock().reduce_session_policy(
            Instant::now(),
            endpoint_policy,
            owner_analysis_pending,
        );
        if let Some(committed_update) = committed_update {
            handle_target_speaker_endpoint_stop(
                &inner_for_speaker,
                session_id,
                &stop_dispatched,
                &clock_for_speaker,
                committed_update,
                endpoint_policy,
            );
        }
    })));
}
