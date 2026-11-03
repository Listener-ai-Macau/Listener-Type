fn complete_pending_tail_clock(now: Instant, quiet_ms: u64) -> SettledTargetEndpointClock {
    use crate::asr::volcengine::{LocalSpeechActivityState, LocalSpeechEvidence, PendingBodyDelivery};
    let speech_end = 20_672;
    let mut clock = automatic_body_clock();
    let mut update = owner_update(speech_end + quiet_ms, speech_end);
    update.provider_audio_duration_ms = Some(20_600);
    clock.latest_update = Some(update);
    clock.latest_visible_body_ends_terminal = Some(true);
    clock.last_body_delivery_at = Some(now - Duration::from_secs(5));
    clock.refresh_pending_body_delivery(Some(PendingBodyDelivery {
        key: "等待最终确认的尾句".into(), updated_at: now,
        recognition_pending: true,
    }), now);
    clock.note_local_vad_evidence_at(LocalSpeechEvidence {
        revision: 1, analyzed_through_ms: speech_end + quiet_ms,
        state: LocalSpeechActivityState::NonSpeech,
        last_detected_speech_end_ms: Some(speech_end), ..Default::default()
    }, now);
    clock
}

#[test]
fn completed_recognition_tail_requests_normal_finalization_without_consuming_words() {
    let now = Instant::now();
    let mut before = complete_pending_tail_clock(now, 2_999);
    assert!(!before.is_due(now, 3_000));
    let mut due = complete_pending_tail_clock(now, 3_000);
    assert!(due.is_due(now, 3_000));
    assert!(due.pending_body_quiet_proposal);
    assert!(due.pending_owner_delivery_active(now), "STOP must preserve final-tail obligations");
    assert!(due.pending_owner_delivery.as_ref().unwrap().recognition_pending);
    assert_eq!(due.pending_owner_delivery.as_ref().unwrap().key, "等待最终确认的尾句");
}

#[test]
fn pending_recognition_tail_keeps_long_guard_for_open_or_unobserved_input() {
    let now = Instant::now();
    for case in 0..5 {
        let mut clock = complete_pending_tail_clock(now, 3_000);
        match case {
            0 => clock.latest_visible_body_ends_terminal = Some(false),
            1 => clock.latest_visible_body_ends_terminal = None,
            2 => clock.last_body_delivery_at = None,
            3 => clock.latest_update.as_mut().unwrap().provider_audio_duration_ms = Some(19_000),
            _ => clock.pending_owner_delivery.as_mut().unwrap().recognition_pending = false,
        }
        assert!(!clock.is_due(now, 3_000), "case {case} has not earned recognition-only finalization");
    }
}

#[test]
fn pending_recognition_tail_rechecks_vad_and_editor_dispatch_before_stop() {
    use crate::asr::volcengine::{LocalSpeechActivityState, LocalSpeechEvidence};
    let now = Instant::now();
    let mut clock = complete_pending_tail_clock(now, 3_000);
    assert!(clock.is_due(now, 3_000));
    assert!(!clock.pending_body_quiet_stop_due(23_672, now + Duration::from_secs(1), 3_000),
        "stale silence must not dispatch STOP");
    clock.note_local_vad_evidence_at(LocalSpeechEvidence {
        revision: 2, analyzed_through_ms: 23_750, activity_epoch: 1,
        pending_speech_start_ms: Some(23_700),
        state: LocalSpeechActivityState::PendingSpeech, ..Default::default()
    }, now);
    assert!(!clock.pending_body_quiet_stop_due(23_750, now, 3_000));
    let mut dispatching = complete_pending_tail_clock(now, 3_000);
    assert!(dispatching.begin_text_submission(now, "正在上屏的正文").is_some());
    assert!(!dispatching.is_due(now, 3_000), "an actual in-flight paste still owns its hold");
}
