#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
fn snapshot_runtime_fixture() -> VolcengineStreamingASR {
    use crate::transcript_boundary::{TimedTranscript, TimedTranscriptWord};
    let asr = uplink_stall_test_asr();
    asr.note_local_speaker_tracking_started_with_owner("开始录音", true);
    let old = Instant::now() - Duration::from_secs(2);
    let timing = TimedTranscript {
        text: "开始录音。本人正文。本人尾句。旁人尾巴。".into(),
        words: [("开始录音。", 100, 500), ("本人正文。", 1000, 1800),
            ("本人尾句。", 2100, 2900), ("旁人尾巴。", 3500, 4100)]
            .into_iter().map(|(text,start,end)| TimedTranscriptWord {
                text:text.into(), start_ms:Some(start), end_ms:Some(end),
            }).collect(),
    };
    let captured_pcm_bytes = 4500 * 32;
    {
        let mut state = asr.state.lock();
        state.is_connected = true;
        state.best_transcript_text = timing.text.clone();
        state.best_transcript_committed_at = Some(old);
        state.best_transcript_timing = Some(timing.clone());
        state.provider_timed_transcript = Some(timing.clone());
        state.recognition_boundary.update(&json!({"utterances":[{
            "text":timing.text, "definite":false,
            "words":timing.words.iter().map(|word|json!({"text":word.text,
                "start_time":word.start_ms,"end_time":word.end_ms})).collect::<Vec<_>>()
        }]}));
        state.qualified_owner_speech_end_ms = Some(2900);
        state.provider_distinct_foreign_audio_ranges = vec![(3200,4500)];
        state.next_input_destination_offset = captured_pcm_bytes;
    }
    *asr.retained_pcm.lock() = vec![0; captured_pcm_bytes as usize];
    let stream = super::super::target_speaker_extraction::TargetSpeakerStream::test_live_owner(None,false,false);
    stream.test_set_source_owner_evidence(vec![(0,3000)],old);
    *asr.target_speaker_stream.lock() = Some(stream);
    let parent_lifecycle_generation = asr.source_snapshot.lock().lifecycle_generation;
    asr.source_snapshot.lock().parent_stream_id = Some(asr.asr_stream_id());
    asr.source_snapshot.lock().result = Some(SourceSnapshotResult {
        anchor: None,
        parent_stream_id:asr.asr_stream_id(), source_epoch:asr.state.lock().owner_delivery_generation,
        parent_lifecycle_generation, child_decoder_id:77, captured_origin_bytes:0,
        captured_pcm_bytes, captured_through_ms:4500, confirmed:timing, completed_at:old,
    });
    asr
}

#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
fn archived_snapshot_runtime_fixture() -> (VolcengineStreamingASR, Value) {
    use crate::transcript_boundary::{TimedTranscript, TimedTranscriptWord};
    let fixture: Value = serde_json::from_str(include_str!("volcengine_snapshot_timing_fixture.json"))
        .expect("archived snapshot timing fixture must remain valid JSON");
    let timed = |value: &Value| TimedTranscript {
        text: value["text"].as_str().unwrap().to_string(),
        words: value["words"].as_array().unwrap().iter().map(|word| TimedTranscriptWord {
            text: word["text"].as_str().unwrap().to_string(),
            start_ms: word["start_ms"].as_u64(),
            end_ms: word["end_ms"].as_u64(),
        }).collect(),
    };
    let ranges = |name: &str| fixture[name].as_array().unwrap().iter().map(|range| {
        let values = range.as_array().unwrap();
        (values[0].as_u64().unwrap(), values[1].as_u64().unwrap())
    }).collect::<Vec<_>>();
    let parent = timed(&fixture["parent_online"]);
    let child = timed(&fixture["child_final"]);
    let owner_ranges = ranges("owner_ranges_ms");
    let foreign_ranges = ranges("foreign_ranges_ms");
    let binding = &fixture["binding"];
    let captured_pcm_bytes = binding["captured_pcm_bytes"].as_u64().unwrap();
    let captured_through_ms = binding["captured_through_ms"].as_u64().unwrap();
    assert_eq!(captured_pcm_bytes / 32, captured_through_ms);

    let asr = uplink_stall_test_asr();
    asr.note_local_speaker_tracking_started_with_owner("开始录音", true);
    let old = Instant::now() - Duration::from_secs(2);
    {
        let mut state = asr.state.lock();
        state.is_connected = true;
        state.best_transcript_text = parent.text.clone();
        state.best_transcript_committed_at = Some(old);
        state.best_transcript_timing = Some(parent.clone());
        state.provider_timed_transcript = Some(parent.clone());
        state.recognition_boundary.update(&json!({"utterances":[{
            "text":parent.text, "definite":false,
            "words":parent.words.iter().map(|word|json!({"text":word.text,
                "start_time":word.start_ms,"end_time":word.end_ms})).collect::<Vec<_>>()
        }]}));
        state.qualified_owner_speech_end_ms = owner_ranges.last().map(|range| range.1);
        state.provider_distinct_foreign_audio_ranges = foreign_ranges;
        state.next_input_destination_offset = captured_pcm_bytes;
    }
    *asr.retained_pcm.lock() = vec![0; captured_pcm_bytes as usize];
    let stream = super::super::target_speaker_extraction::TargetSpeakerStream::test_live_owner(None,false,false);
    stream.test_set_source_owner_evidence(owner_ranges, old);
    *asr.target_speaker_stream.lock() = Some(stream);
    let parent_lifecycle_generation = asr.source_snapshot.lock().lifecycle_generation;
    asr.source_snapshot.lock().parent_stream_id = Some(asr.asr_stream_id());
    asr.source_snapshot.lock().result = Some(SourceSnapshotResult {
        anchor: None,
        parent_stream_id: asr.asr_stream_id(), parent_lifecycle_generation,
        source_epoch: asr.state.lock().owner_delivery_generation,
        child_decoder_id: binding["child_decoder_id"].as_u64().unwrap(),
        captured_origin_bytes: binding["captured_origin_bytes"].as_u64().unwrap(),
        captured_pcm_bytes, captured_through_ms, confirmed: child, completed_at: old,
    });
    (asr, fixture)
}

#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
#[test]
fn lexical_snapshot_unlocks_only_the_same_owned_words_without_touching_endpoint_or_primary() {
    let asr = snapshot_runtime_fixture();
    let primary_before = asr.state.lock().best_transcript_text.clone();
    let candidate = asr.selected_live_owner_candidate().unwrap();
    assert!(candidate.recognition_confirmed_prefix.is_none());
    let pending_before = asr.pending_owner_delivery_after("").unwrap();
    let delivered = asr.pause_early_delivery_ledger_snapshot_after(Duration::ZERO, "").unwrap();
    assert_eq!(delivered.text, "开始录音。本人正文。本人尾句。");
    let pending_after = asr.pending_owner_delivery_after("").unwrap();
    assert_eq!(pending_before, pending_after, "child completion cannot refresh the owner settling clock");
    let state = asr.state.lock();
    assert_eq!(state.best_transcript_text, primary_before, "no short child overwrites final candidates");
    assert_eq!(state.qualified_owner_speech_end_ms, Some(2900));
    assert!(state.recognition_boundary.confirmed_prefix("开始录音。本人正文。", state.best_transcript_timing.as_ref()).is_none(),
        "parent decoder must not inherit the child certificate");
}

#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
#[test]
fn archived_same_pcm_snapshot_uses_child_timing_and_rendering_without_parent_span_overlap() {
    use crate::transcript_boundary::content_key;
    let (asr, fixture) = archived_snapshot_runtime_fixture();
    let parent_first = &fixture["parent_online"]["words"][0];
    let child_second = &fixture["child_final"]["words"][1];
    assert_eq!(parent_first["text"], "开始");
    assert_eq!(child_second["text"], "始");
    assert!(child_second["end_ms"].as_u64().unwrap()
        < parent_first["start_ms"].as_u64().unwrap(),
        "the archived online and final decoder spans intentionally do not overlap");

    let parent = asr.selected_live_owner_candidate().unwrap();
    assert!(parent.recognition_confirmed_prefix.is_none());
    let snapshot = asr.source_snapshot_delivery_candidate(&parent).unwrap();
    let expected = fixture["expected_child_owned_rendering"].as_str().unwrap();
    assert_eq!(snapshot.text, expected,
        "the irreversible candidate must keep the child final punctuation rendering");
    assert_eq!(snapshot.recognition_confirmed_prefix.as_deref(), Some(expected));
    assert_eq!(snapshot.stability_source.authority, "source_snapshot");
    assert_eq!(snapshot.stability_source.decoder,
        fixture["binding"]["child_decoder_id"].as_u64().unwrap());
    assert!(!content_key(&snapshot.text).contains(
        &content_key(fixture["expected_foreign_text"].as_str().unwrap())));
    assert_eq!(
        asr.pause_early_delivery_ledger_snapshot_after(Duration::ZERO, "").unwrap().text,
        expected,
    );
    assert!(asr.selected_live_owner_candidate().unwrap().recognition_confirmed_prefix.is_none(),
        "the parent recognition boundary must remain provisional");
}

#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
#[test]
fn archived_child_word_under_a_current_foreign_veto_cannot_authorize_delivery() {
    let (asr, _) = archived_snapshot_runtime_fixture();
    // This interval intersects the child decoder's first word (280..520ms),
    // while remaining before the parent online token (680..760ms). The veto
    // must therefore be checked in the child's own time coordinate.
    asr.state.lock().provider_distinct_foreign_audio_ranges.push((300, 301));
    let parent = asr.selected_live_owner_candidate().unwrap();
    assert!(asr.source_snapshot_delivery_candidate(&parent).is_none());
    assert!(asr.pause_early_delivery_ledger_snapshot_after(Duration::ZERO, "").is_none());
}

#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
#[test]
fn injected_snapshot_requires_exact_origin_length_and_lifecycle_binding() {
    let asr = snapshot_runtime_fixture();
    let parent = asr.selected_live_owner_candidate().unwrap();
    assert!(asr.source_snapshot_delivery_candidate(&parent).is_some());

    asr.source_snapshot.lock().result.as_mut().unwrap().captured_origin_bytes = 32;
    assert!(asr.source_snapshot_delivery_candidate(&parent).is_none());
    asr.source_snapshot.lock().result.as_mut().unwrap().captured_origin_bytes = 0;

    asr.source_snapshot.lock().result.as_mut().unwrap().captured_pcm_bytes -= 32;
    assert!(asr.source_snapshot_delivery_candidate(&parent).is_none());
    asr.source_snapshot.lock().result.as_mut().unwrap().captured_pcm_bytes += 32;

    asr.retained_pcm.lock().pop();
    assert!(asr.source_snapshot_delivery_candidate(&parent).is_none());
    asr.retained_pcm.lock().push(0);

    let stale = asr.source_snapshot.lock().lifecycle_generation.wrapping_sub(1);
    asr.source_snapshot.lock().result.as_mut().unwrap().parent_lifecycle_generation = stale;
    assert!(asr.source_snapshot_delivery_candidate(&parent).is_none());
}

#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
#[test]
fn lexical_snapshot_respects_late_foreign_veto() {
    let asr = snapshot_runtime_fixture();
    asr.state.lock().provider_distinct_foreign_audio_ranges.push((2200,2400));
    let delivered = asr.pause_early_delivery_ledger_snapshot_after(Duration::ZERO, "").unwrap();
    assert_eq!(delivered.text, "开始录音。本人正文。");
}

#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
#[test]
fn lexical_snapshot_short_prefix_does_not_hide_owner_continuation() {
    use crate::transcript_boundary::content_key;
    let asr = snapshot_runtime_fixture();
    {
        let mut slot = asr.source_snapshot.lock();
        let result = slot.result.as_mut().unwrap();
        result.confirmed = snapshot_timed_prefix(&result.confirmed,
            &content_key("开始录音。本人正文。")).unwrap();
    }
    let delivered = asr.pause_early_delivery_ledger_snapshot_after(Duration::ZERO, "").unwrap();
    assert_eq!(delivered.text,"开始录音。本人正文。");
    assert_eq!(asr.pending_owner_delivery_after(&content_key("本人正文。")).unwrap().0,
        content_key("本人尾句。"));
    assert_eq!(asr.selected_live_owner_candidate().unwrap().text,
        "开始录音。本人正文。本人尾句。");
}

#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
#[test]
fn lexical_snapshot_cannot_cross_reopen_revocation_stop_or_lost_owner_permission() {
    let asr = snapshot_runtime_fixture();
    let parent = asr.selected_live_owner_candidate().unwrap();
    assert!(asr.source_snapshot_delivery_candidate(&parent).is_some());
    asr.state.lock().owner_delivery_generation += 1;
    assert!(asr.source_snapshot_delivery_candidate(&parent).is_none());
    asr.state.lock().owner_delivery_generation -= 1;
    asr.asr_stream_id.fetch_add(1, Ordering::SeqCst);
    assert!(asr.source_snapshot_delivery_candidate(&parent).is_none());
    asr.asr_stream_id.fetch_sub(1, Ordering::SeqCst);
    asr.state.lock().finishing = true;
    assert!(asr.source_snapshot_delivery_candidate(&parent).is_none());
    asr.state.lock().finishing = false;
    asr.state.lock().local_wake_owner_verified = false;
    assert!(asr.source_snapshot_delivery_candidate(&parent).is_none());
}

#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
#[test]
fn lexical_snapshot_cleanup_seals_claims_and_never_reuses_request_ids() {
    let asr = snapshot_runtime_fixture();
    {
        let mut slot = asr.source_snapshot.lock();
        slot.next_request_id = 8;
        slot.flight = Some(SourceSnapshotFlight { anchor_generation: None, request_id:8,parent_stream_id:asr.asr_stream_id(),source_epoch:0,abort:None });
        slot.attempted_through_ms = 4500;
        slot.replayed_pcm_bytes = 4500*32;
    }
    asr.seal_source_snapshot();
    {
        let slot = asr.source_snapshot.lock();
        assert!(slot.closed && slot.flight.is_none() && slot.result.is_none());
    }
    asr.reset_source_snapshot();
    let slot = asr.source_snapshot.lock();
    assert!(slot.closed && slot.parent_stream_id.is_none());
    assert_eq!(slot.next_request_id,8);
    assert_eq!(slot.attempted_through_ms,0);
    assert_eq!(slot.replayed_pcm_bytes,0);
}

#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
#[test]
fn delivery_evidence_keeps_archived_child_times_instead_of_equal_parent_words() {
    let (asr, fixture) = archived_snapshot_runtime_fixture();
    let delivered = asr.pause_early_delivery_evidence_after(Duration::ZERO, "").unwrap();
    let evidence = delivered.timing.unwrap();
    assert_eq!(evidence.timing.text, delivered.text);
    assert_eq!(evidence.source.decoder_stream_id, fixture["binding"]["child_decoder_id"].as_u64().unwrap());
    assert_eq!(evidence.source.capture_stream_id, asr.asr_stream_id());
    assert_eq!(evidence.timing.words[0].start_ms, Some(280));
    assert_eq!(evidence.timing.words[0].text, "开");
    assert_eq!(asr.provider_timed_transcript_snapshot().unwrap().words[0].start_ms, Some(680));
    // Later provider updates cannot change the already returned evidence.
    asr.state.lock().provider_timed_transcript = None;
    assert_eq!(evidence.timing.words[0].start_ms, Some(280));
    let generation = evidence.source.capture_generation;
    asr.cancel_source_snapshot();
    assert_eq!(asr.source_snapshot.lock().capture_generation, generation,
        "worker cancellation is not a new audio capture");
    asr.reset_source_snapshot();
    assert_ne!(asr.source_snapshot.lock().capture_generation, generation,
        "reopening the same object invalidates all old submitted PCM boundaries");
}

#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
#[test]
fn delivery_evidence_retains_independent_decoder_times_and_does_not_fill_missing_times() {
    use crate::transcript_boundary::{TimedTranscript, TimedTranscriptWord};
    let parent = uplink_stall_test_asr();
    let child = Arc::new(uplink_stall_test_asr());
    let text = "本人重复的一句话。";
    let old = Instant::now() - Duration::from_secs(2);
    let timing = |start| TimedTranscript { text: text.into(), words: vec![TimedTranscriptWord {
        text: text.into(), start_ms: Some(start), end_ms: Some(start + 400),
    }] };
    {
        let mut state = parent.state.lock();
        state.best_transcript_text = text.into();
        state.best_transcript_timing = Some(timing(8_000));
        state.provider_timed_transcript = state.best_transcript_timing.clone();
        state.best_transcript_committed_at = Some(old);
    }
    {
        let mut state = child.state.lock();
        state.best_transcript_text = text.into();
        state.best_transcript_timing = Some(timing(1_000));
        state.best_transcript_committed_at = Some(old);
    }
    *parent.target_speaker_stream.lock() = Some(
        super::super::target_speaker_extraction::TargetSpeakerStream::test_live_owner(Some(Arc::clone(&child)), true, true));
    let delivered = parent.pause_early_delivery_evidence_after(Duration::ZERO, "").unwrap();
    let evidence = delivered.timing.unwrap();
    assert_eq!(evidence.source.decoder_stream_id, child.asr_stream_id());
    assert_eq!(evidence.source.capture_stream_id, parent.asr_stream_id());
    assert_eq!(evidence.timing.words[0].start_ms, Some(1_000));
    child.state.lock().best_transcript_timing = None;
    let untimed = parent.pause_early_delivery_evidence_after(Duration::ZERO, "").unwrap();
    assert_eq!(untimed.text, text, "legacy untimed delivery keeps its existing rolling gate");
    assert!(untimed.timing.is_none(), "matching parent text cannot fill absent child coordinates");
}

#[test]
fn delivery_confirmation_cannot_borrow_equal_words_from_another_occurrence() {
    use crate::transcript_boundary::{TimedTranscript, TimedTranscriptWord};
    let asr = uplink_stall_test_asr();
    let text = "重复的句子";
    let timing = |start| TimedTranscript { text: text.into(), words: vec![TimedTranscriptWord {
        text: text.into(), start_ms: Some(start), end_ms: Some(start + 400),
    }] };
    {
        let mut state = asr.state.lock();
        state.live_owner_delivery = Some(TranscriptCandidate {text: text.into(), ..Default::default()});
        state.live_owner_delivery_timing = Some(timing(4_000));
        state.live_owner_delivery_updated_at = Some(Instant::now() - Duration::from_secs(2));
        state.provider_timed_transcript = Some(timing(100));
        state.recognition_boundary.update(&json!({"utterances":[
            {"text":text,"definite":true,"words":[{"text":text,"start_time":100,"end_time":500}]},
            {"text":text,"definite":false,"words":[{"text":text,"start_time":4000,"end_time":4400}]}
        ]}));
    }
    assert!(asr.pause_early_delivery_evidence_after(Duration::ZERO, "").is_none(),
        "earlier confirmed occurrence cannot certify the currently selected later occurrence");
    asr.state.lock().recognition_boundary.update(&json!({"utterances":[
        {"text":text,"definite":true,"words":[{"text":text,"start_time":4000,"end_time":4400}]}
    ]}));
    let delivered = asr.pause_early_delivery_evidence_after(Duration::ZERO, "").unwrap();
    assert_eq!(delivered.text, text);
    assert_eq!(delivered.timing.unwrap().timing.words[0].start_ms, Some(4_000));
}

#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
#[test]
fn delivery_evidence_selected_before_stop_cancel_or_reopen_is_discarded() {
    // Freeze execution at the production selection/return boundary. The
    // intervening lifecycle operation models STOP or reopen winning that race.
    for operation in ["stop", "cancel", "reopen"] {
        let asr = snapshot_runtime_fixture();
        let primary = asr.selected_live_owner_candidate().unwrap();
        let selected = asr.source_snapshot_delivery_candidate(&primary).unwrap();
        let mut unobserved = selected.clone();
        unobserved.owner_permission = LiveOwnerPermission::Unobserved;
        let binding = {
            let slot = asr.source_snapshot.lock();
            (asr.asr_stream_id(), slot.capture_generation, slot.lifecycle_generation)
        };
        match operation {
            "stop" => asr.seal_source_snapshot(),
            "cancel" => asr.cancel_source_snapshot(),
            "reopen" => asr.reset_source_snapshot(),
            _ => unreachable!(),
        }
        assert!(asr.delivery_evidence_from_selected(Duration::ZERO, "", selected, binding).is_none(),
            "a previously selected child cannot cross {operation} and authorize a new submission");
        assert!(asr.state.lock().pause_early_stability_source.is_none(),
            "a stale selection must not write its maturity into the next lifecycle");
        let current_generation = asr.state.lock().owner_delivery_generation;
        assert!(asr.delivery_evidence_from_selected(Duration::ZERO, "", unobserved, binding).is_none());
        assert_eq!(asr.state.lock().owner_delivery_generation, current_generation,
            "a stale unclassified selection cannot revoke the new lifecycle's permission");
    }
}
#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
fn cropped_snapshot_runtime_fixture() -> (VolcengineStreamingASR, LiveLedgerAnchor) {
    use crate::transcript_boundary::{content_key, TimedTranscript, TimedTranscriptWord};
    let asr = snapshot_runtime_fixture();
    let old = Instant::now() - Duration::from_secs(2);
    let tokens = [("开始录音。",100,500),("已经提交。",55000,55800),
        ("继续第一句。",63000,64000),("继续第二句。",66000,67000),("旁人尾巴。",69000,70000)];
    let parent = TimedTranscript { text: tokens.iter().map(|word|word.0).collect(),
        words: tokens.iter().map(|&(text,start,end)|TimedTranscriptWord {
            text:text.into(),start_ms:Some(start),end_ms:Some(end),
        }).collect() };
    let capture_bytes = 71500 * 32;
    {
        let mut state = asr.state.lock();
        state.best_transcript_text = parent.text.clone();
        state.best_transcript_timing = Some(parent.clone());
        state.provider_timed_transcript = Some(parent.clone());
        state.best_transcript_committed_at = Some(old);
        state.recognition_boundary.update(&json!({"utterances":[{
            "text":parent.text,"definite":false,
            "words":parent.words.iter().map(|word|json!({"text":word.text,
                "start_time":word.start_ms,"end_time":word.end_ms})).collect::<Vec<_>>()
        }]}));
        state.qualified_owner_speech_end_ms = Some(67000);
        state.provider_distinct_foreign_audio_ranges = vec![(68000,71500)];
        state.next_input_destination_offset = capture_bytes;
    }
    *asr.retained_pcm.lock() = vec![0; capture_bytes as usize];
    asr.target_speaker_stream.lock().as_ref().unwrap().test_set_source_owner_evidence(vec![(0,68000)],old);
    let source = LiveDeliverySource { capture_stream_id:asr.asr_stream_id(),
        capture_generation:asr.source_snapshot.lock().capture_generation,
        decoder_stream_id:asr.asr_stream_id(), owner_generation:asr.state.lock().owner_delivery_generation };
    let anchor = LiveLedgerAnchor { source, generation:1,source_key:content_key("已经提交。"),
        source_text:"开始录音。已经提交。".into(),boundary:parent.delivery_boundary(&content_key("已经提交。")).unwrap() };
    let mut child = TimedTranscript { text:tokens[1..].iter().map(|word|word.0).collect(),words:parent.words[1..].to_vec() };
    for word in &mut child.words {
        word.start_ms = word.start_ms.map(|time|time-54000);
        word.end_ms = word.end_ms.map(|time|time-54000);
    }
    let mut slot = asr.source_snapshot.lock();
    slot.result = Some(SourceSnapshotResult { anchor:Some(anchor.clone()), parent_stream_id:asr.asr_stream_id(),
        parent_lifecycle_generation:slot.lifecycle_generation, source_epoch:source.owner_generation,
        child_decoder_id:901,captured_origin_bytes:54000*32,captured_pcm_bytes:(71500-54000)*32,
        captured_through_ms:71500,confirmed:child,completed_at:old });
    drop(slot);
    (asr,anchor)
}

#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
#[test]
fn cropped_snapshot_after_sixty_seconds_has_only_owned_child_suffix_timing() {
    use crate::transcript_boundary::content_key;
    let (asr,anchor) = cropped_snapshot_runtime_fixture();
    let before = asr.state.lock().best_transcript_text.clone();
    let next = asr.pause_early_ledger_continuation_after(Duration::ZERO,&anchor).unwrap();
    assert_eq!(next.suffix.timing.text,"继续第一句。继续第二句。");
    assert_eq!(next.suffix.timing.words.len(),2);
    assert_eq!(next.suffix.timing.words[0].start_ms,Some(63000));
    assert_eq!(next.suffix.source.decoder_stream_id,901);
    assert_eq!(next.source_text(),"开始录音。已经提交。继续第一句。继续第二句。");
    assert!(next.owner_caught_up);
    let advanced = next.advance(&content_key("继续第一句。"),&content_key("已经提交。继续第一句。")).unwrap();
    assert_eq!(advanced.generation,2);
    assert_eq!(advanced.boundary.range_ms(),(63000,64000));
    assert_eq!(advanced.source_text,"开始录音。已经提交。继续第一句。");
    assert!(next.advance(&content_key("继续第一"),"ignored").is_none(),"never advance inside a token");
    assert_eq!(asr.state.lock().best_transcript_text,before,"child suffix cannot replace the parent final");
    assert_eq!(asr.state.lock().qualified_owner_speech_end_ms,Some(67000));
    assert!(asr.pause_early_ledger_continuation_after(Duration::ZERO,&advanced).is_none(),"old result cannot be reused after a successful advance");
    // A second independently finalized crop can continue from the newly
    // submitted child occurrence without replaying its first sentence.
    {
        let mut slot = asr.source_snapshot.lock();
        let result = slot.result.as_mut().unwrap();
        result.anchor = Some(advanced.clone());
        result.child_decoder_id = 902;
        result.captured_origin_bytes = 62000*32;
        result.captured_pcm_bytes = (71500-62000)*32;
        result.confirmed.text = "继续第一句。继续第二句。旁人尾巴。".into();
        result.confirmed.words.remove(0);
        for word in &mut result.confirmed.words {
            word.start_ms = word.start_ms.map(|time|time-8000);
            word.end_ms = word.end_ms.map(|time|time-8000);
        }
    }
    let second = asr.pause_early_ledger_continuation_after(Duration::ZERO,&advanced).unwrap();
    assert_eq!(second.suffix.timing.text,"继续第二句。");
    assert_eq!(second.suffix.timing.words[0].start_ms,Some(66000));
    let final_anchor = second.advance(&content_key("继续第二句。"),&content_key("已经提交。继续第一句。继续第二句。")).unwrap();
    assert_eq!(final_anchor.generation,3);
    assert_eq!(final_anchor.source_text,"开始录音。已经提交。继续第一句。继续第二句。");
}

#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
#[test]
fn cropped_snapshot_rechecks_foreign_permissions_and_current_owner_frontier() {
    let (asr,anchor) = cropped_snapshot_runtime_fixture();
    let earlier = asr.pause_early_ledger_continuation_after(Duration::ZERO,&anchor).unwrap();
    asr.retained_pcm.lock().pop();
    assert!(!asr.ledger_continuation_is_current(&earlier),
        "a retained append/destination-ledger gap cannot dispatch a cached quiet suffix");
    asr.retained_pcm.lock().push(0);
    assert!(asr.ledger_continuation_is_current(&earlier));
    asr.state.lock().destination_ledger_incomplete = true;
    assert!(!asr.ledger_continuation_is_current(&earlier),
        "incomplete destination coordinates invalidate the child PCM binding");
    asr.state.lock().destination_ledger_incomplete = false;
    assert!(asr.ledger_continuation_is_current(&earlier));
    {
        let mut state = asr.state.lock();
        state.local_wake_owner_verified = false;
        state.local_body_owner_verified = false;
    }
    assert!(!asr.ledger_continuation_is_current(&earlier),
        "a cached child cannot outlive the enrolled owner permission that selected it");
    asr.state.lock().local_wake_owner_verified = true;
    assert!(asr.ledger_continuation_is_current(&earlier));
    asr.state.lock().qualified_owner_speech_end_ms = Some(72_000);
    assert!(!asr.ledger_continuation_is_current(&earlier),
        "new approved owner audio beyond the child horizon revokes cached owner_caught_up");

    let (asr,anchor) = cropped_snapshot_runtime_fixture();
    let earlier = asr.pause_early_ledger_continuation_after(Duration::ZERO,&anchor).unwrap();
    asr.state.lock().provider_distinct_foreign_audio_ranges.push((66500,66600));
    assert!(!asr.ledger_continuation_is_current(&earlier),"a new foreign veto invalidates an already returned suffix even before an epoch transition");
    let (asr,anchor) = cropped_snapshot_runtime_fixture();
    asr.state.lock().provider_distinct_foreign_audio_ranges.push((66500,66600));
    let next = asr.pause_early_ledger_continuation_after(Duration::ZERO,&anchor).unwrap();
    assert_eq!(next.suffix.timing.text,"继续第一句。");
    assert!(asr.ledger_continuation_is_current(&next));
    asr.state.lock().owner_delivery_generation += 1;
    assert!(!asr.ledger_continuation_is_current(&next));
    assert!(asr.pause_early_ledger_continuation_after(Duration::ZERO,&anchor).is_none());
    let (asr,anchor) = cropped_snapshot_runtime_fixture();
    asr.state.lock().provider_distinct_foreign_audio_ranges.push((63500,63600));
    assert!(asr.pause_early_ledger_continuation_after(Duration::ZERO,&anchor).is_none(),"do not jump a rejected first suffix to later owner words");
    let (asr,anchor) = cropped_snapshot_runtime_fixture();
    asr.state.lock().qualified_owner_speech_end_ms = Some(72000);
    let next = asr.pause_early_ledger_continuation_after(Duration::ZERO,&anchor).unwrap();
    assert!(!next.owner_caught_up,"unrecognized newer owner audio prevents quiet completion");
}

#[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
#[test]
fn cropped_snapshot_cannot_cross_ledger_generation_origin_reopen_or_stop() {
    let (asr,anchor) = cropped_snapshot_runtime_fixture();
    let mut newer = anchor.clone(); newer.generation += 1;
    assert!(asr.pause_early_ledger_continuation_after(Duration::ZERO,&newer).is_none());
    let mut different = anchor.clone(); different.source_key.push('另');
    assert!(asr.pause_early_ledger_continuation_after(Duration::ZERO,&different).is_none());
    asr.source_snapshot.lock().result.as_mut().unwrap().captured_origin_bytes += 32;
    assert!(asr.pause_early_ledger_continuation_after(Duration::ZERO,&anchor).is_none());
    asr.source_snapshot.lock().result.as_mut().unwrap().captured_origin_bytes -= 32;
    let next = asr.pause_early_ledger_continuation_after(Duration::ZERO,&anchor).unwrap();
    asr.seal_source_snapshot();
    assert!(!asr.ledger_continuation_is_current(&next));
    assert!(asr.pause_early_ledger_continuation_after(Duration::ZERO,&anchor).is_none());
    asr.reset_source_snapshot();
    asr.source_snapshot.lock().closed = false;
    assert!(asr.pause_early_ledger_continuation_after(Duration::ZERO,&anchor).is_none());
}

#[test]
fn cropped_snapshot_planner_requires_exact_capture_context_and_transport_budget() {
    assert_eq!(snapshot_clip_bounds(72000*32,72000*32,70000,Some((55000,55800)),0),Some((54000*32,72000)));
    assert!(snapshot_clip_bounds(72000*32,72000*32,70000,None,0).is_none(),"whole prefix is bounded");
    assert!(snapshot_clip_bounds(72000*32,72000*32,71500,Some((55000,55800)),0).is_none(),"captured context is required");
    assert!(snapshot_clip_bounds(72000*32,72000*32+2,70000,Some((55000,55800)),0).is_none(),"unknown retained origin cannot be guessed");
    assert!(snapshot_clip_bounds(72000*32,72000*32,70000,Some((5000,5800)),0).is_none(),"stale seam cannot replay unbounded PCM");
    assert!(snapshot_clip_bounds(72000*32,72000*32,70000,Some((55000,55800)),2*72000*32).is_none());
    assert!(snapshot_clip_bounds(72000*32,72000*32,70000,Some((70000,71000)),0).is_none(),"future boundary is not already submitted source");
}

#[cfg(target_os = "windows")]
fn admitted_tail_snapshot_fixture() -> (VolcengineStreamingASR, LiveLedgerAnchor) {
    use crate::transcript_boundary::{content_key, TimedTranscript, TimedTranscriptWord};
    let asr = uplink_stall_test_asr().with_owner_activity_only(true);
    let old = Instant::now() - Duration::from_secs(3);
    let tokens = [("开始录音。", 100, 500), ("前段已经上屏。", 10_000, 11_000),
        ("尾句继续写入。", 12_000, 14_500)];
    let parent = TimedTranscript {
        text: tokens.iter().map(|word| word.0).collect(),
        words: tokens.iter().map(|&(text, start, end)| TimedTranscriptWord {
            text: text.into(), start_ms: Some(start), end_ms: Some(end),
        }).collect(),
    };
    {
        let mut state = asr.state.lock();
        state.is_connected = true;
        state.best_transcript_text = parent.text.clone();
        state.best_transcript_timing = Some(parent.clone());
        state.provider_timed_transcript = Some(parent.clone());
        state.best_transcript_committed_at = Some(old);
        state.next_input_destination_offset = 20_000 * 32;
        state.wake_speaker_phrase = Some("开始录音".into());
        state.recognition_boundary.update(&json!({"utterances":tokens.iter().enumerate().map(|(i, word)|
            json!({"text":word.0,"definite":i < 2,"words":[{"text":word.0,
                "start_time":word.1,"end_time":word.2}]})).collect::<Vec<_>>()}));
    }
    *asr.retained_pcm.lock() = vec![0; 20_000 * 32];
    let source = LiveDeliverySource { capture_stream_id: asr.asr_stream_id(),
        capture_generation: asr.source_snapshot.lock().capture_generation,
        decoder_stream_id: asr.asr_stream_id(), owner_generation: asr.state.lock().owner_delivery_generation };
    let anchor = LiveLedgerAnchor { source, generation: 1, source_key: content_key("前段已经上屏。"),
        source_text: "开始录音。前段已经上屏。".into(),
        boundary: parent.delivery_boundary(&content_key("开始录音。前段已经上屏。")).unwrap() };
    let child = TimedTranscript {
        text: tokens[1..].iter().map(|word| word.0).collect(),
        words: parent.words[1..].iter().cloned().map(|mut word| {
            word.start_ms = word.start_ms.map(|at| at - 9_000);
            word.end_ms = word.end_ms.map(|at| at - 9_000);
            word
        }).collect(),
    };
    {
        let mut slot = asr.source_snapshot.lock();
        slot.closed = false;
        slot.parent_stream_id = Some(asr.asr_stream_id());
        slot.result = Some(SourceSnapshotResult { anchor: Some(anchor.clone()),
            parent_stream_id: asr.asr_stream_id(), parent_lifecycle_generation: slot.lifecycle_generation,
            source_epoch: source.owner_generation, child_decoder_id: 99,
            captured_origin_bytes: 9_000 * 32, captured_pcm_bytes: 11_000 * 32,
            captured_through_ms: 20_000, confirmed: child, completed_at: Instant::now() });
    }
    (asr, anchor)
}

#[test]
#[cfg(target_os = "windows")]
fn admitted_pending_tail_uses_confirmed_increment_before_capture_stop() {
    use crate::transcript_boundary::content_key;
    let (asr, anchor) = admitted_tail_snapshot_fixture();
    let pending = asr.pending_body_delivery_after(&anchor.source_key).unwrap();
    assert!(pending.recognition_pending, "the primary still awaits its utterance finality");
    let before = asr.state.lock().best_transcript_text.clone();
    let continued = asr.pause_early_ledger_continuation_after(Duration::from_secs(1), &anchor)
        .expect("a fresh child recognition certificate must not wait for whole-capture STOP or age again");
    assert_eq!(continued.suffix.timing.text, "尾句继续写入。");
    assert!(continued.owner_caught_up);
    assert!(asr.ledger_continuation_is_current(&continued));
    assert_eq!(continued.source_text(), before);
    assert_eq!(asr.state.lock().best_transcript_text, before, "the child cannot overwrite final arbitration");
    assert_eq!(asr.pending_body_delivery_after(&anchor.source_key).unwrap().key, pending.key,
        "a recognition certificate is not itself an editor submission");
    { let state = asr.state.lock(); assert!(state.is_connected && !state.finishing); }
    assert!(asr.pending_body_delivery_after(&content_key(&continued.source_text())).is_none(),
        "the ordinary submitted-source cursor must prevent another tail dispatch");
}

#[test]
#[cfg(target_os = "windows")]
fn admitted_tail_certificate_rejects_new_identity_capture_and_source_mismatches() {
    for case in 0..9 {
        let (asr, mut anchor) = admitted_tail_snapshot_fixture();
        match case {
            0 => asr.state.lock().local_speaker_tracking_enabled = true,
            1 => asr.state.lock().owner_isolation_frozen = true,
            2 => asr.state.lock().destination_ledger_incomplete = true,
            3 => asr.state.lock().owner_delivery_generation += 1,
            4 => anchor.generation += 1,
            5 => asr.source_snapshot.lock().result.as_mut().unwrap().captured_origin_bytes += 32,
            6 => asr.source_snapshot.lock().closed = true,
            7 => asr.state.lock().finishing = true,
            _ => { asr.retained_pcm.lock().pop(); },
        }
        assert!(asr.pause_early_ledger_continuation_after(Duration::ZERO, &anchor).is_none(), "case={case}");
    }
}

#[test]
#[cfg(target_os = "windows")]
fn admitted_tail_revalidation_rejects_cancel_reopen_and_revised_occurrences() {
    for case in 0..5 {
        let (asr, anchor) = admitted_tail_snapshot_fixture();
        let continued = asr.pause_early_ledger_continuation_after(Duration::ZERO, &anchor).unwrap();
        match case {
            0 => asr.seal_source_snapshot(),
            1 => asr.reset_source_snapshot(),
            2 => asr.state.lock().local_speaker_tracking_enabled = true,
            3 => asr.state.lock().owner_delivery_generation += 1,
            _ => {
                let mut state = asr.state.lock();
                state.best_transcript_text = "另一段完全不同的文字。".into();
                state.best_transcript_timing.as_mut().unwrap().words.last_mut().unwrap().text = "不同尾句。".into();
            },
        }
        assert!(!asr.ledger_continuation_is_current(&continued), "case={case}");
    }
}

#[test]
#[cfg(target_os = "windows")]
fn admitted_tail_worker_claims_only_submitted_suffix_with_existing_replay_budget() {
    // This current-thread future never yields. The real request planner is
    // exercised, then its task is aborted before any child/socket is polled.
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(async {
        let (asr, anchor) = admitted_tail_snapshot_fixture();
        asr.state.lock().runtime = Some(Handle::current());
        let candidate = asr.selected_live_owner_candidate().unwrap();
        asr.maybe_start_source_snapshot_from(&candidate, None);
        assert!(asr.source_snapshot.lock().flight.is_none(), "ordinary mode cannot replay the whole recording");
        asr.maybe_start_source_snapshot_from(&candidate, Some(&anchor));
        let request = {
            let slot = asr.source_snapshot.lock();
            let flight = slot.flight.as_ref().expect("a stalled ordinary tail must reach the real worker planner");
            assert_eq!(flight.anchor_generation, Some(anchor.generation));
            assert_eq!(slot.attempted_through_ms, 20_000);
            assert_eq!(slot.replayed_pcm_bytes, 11_000 * 32, "only 9s..20s, not the 0s..20s prefix");
            flight.request_id
        };
        asr.maybe_start_source_snapshot_from(&candidate, Some(&anchor));
        assert_eq!(asr.source_snapshot.lock().flight.as_ref().unwrap().request_id, request,
            "every watchdog poll must reuse the in-flight request");
        asr.seal_source_snapshot();
        assert!(asr.source_snapshot.lock().flight.is_none());

        let (limited, anchor) = admitted_tail_snapshot_fixture();
        limited.state.lock().runtime = Some(Handle::current());
        limited.source_snapshot.lock().replayed_pcm_bytes = 2 * 20_000 * 32;
        let candidate = limited.selected_live_owner_candidate().unwrap();
        limited.maybe_start_source_snapshot_from(&candidate, Some(&anchor));
        assert!(limited.source_snapshot.lock().flight.is_none(), "the existing total replay budget must remain binding");
    });
}

#[cfg(target_os = "windows")]
fn grow_admitted_tail(asr: &VolcengineStreamingASR, fresh: Instant) -> String {
    let mut state=asr.state.lock();
    let before=state.best_transcript_text.clone();
    state.best_transcript_text.push_str("新的词还在增加。");
    state.best_transcript_committed_at=Some(fresh);
    let mut timing=state.best_transcript_timing.clone().unwrap();
    timing.text=state.best_transcript_text.clone();
    timing.words.push(crate::transcript_boundary::TimedTranscriptWord {
        text:"新的词还在增加。".into(),start_ms:Some(18_000),end_ms:Some(19_500)});
    state.best_transcript_timing=Some(timing.clone());
    state.provider_timed_transcript=Some(timing);
    state.pause_early_recent_revisions.push_back((fresh-Duration::from_secs(2),before.clone()));
    before
}

#[test]
#[cfg(target_os = "windows")]
fn admitted_growing_tail_plans_mature_prefix_without_waiting_for_whole_sentence() {
    let runtime=tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(async {
        let (asr,anchor)=admitted_tail_snapshot_fixture();
        grow_admitted_tail(&asr,Instant::now());
        asr.state.lock().runtime=Some(Handle::current());
        asr.source_snapshot.lock().result=None;
        let candidate=asr.selected_live_owner_candidate().unwrap();
        assert!(candidate.updated_at.elapsed()<Duration::from_millis(500));
        assert_eq!(admitted_snapshot_stable_end(&mut asr.state.lock(),&candidate,&anchor,Instant::now()),Some(14_500));
        asr.maybe_start_source_snapshot_from(&candidate,Some(&anchor));
        let slot=asr.source_snapshot.lock();
        assert!(slot.flight.is_some(),"new words must not postpone recognition of a prefix unchanged for the existing maturity window");
        assert_eq!(slot.replayed_pcm_bytes,11_000*32);
        drop(slot);
        asr.seal_source_snapshot(); // no task is yielded, so no socket is opened
    });
}

#[test]
#[cfg(target_os = "windows")]
fn admitted_confirmed_continuation_streams_while_primary_tail_keeps_growing() {
    let (asr,anchor)=admitted_tail_snapshot_fixture();
    grow_admitted_tail(&asr,Instant::now());
    let continued=asr.pause_early_ledger_continuation_after(Duration::from_secs(1),&anchor)
        .expect("an independently confirmed same-source continuation does not wait for the current full text to stop changing");
    assert_eq!(continued.suffix.timing.text,"尾句继续写入。");
    assert!(!continued.owner_caught_up,"the new unconfirmed growth is still pending");
    assert!(asr.ledger_continuation_is_current(&continued));
    let next=continued.advance(&crate::transcript_boundary::content_key("尾句继续写入。"),
        &crate::transcript_boundary::content_key(&continued.source_text())).unwrap();
    assert_eq!(asr.pending_body_delivery_after(&next.source_key).unwrap().key,
        crate::transcript_boundary::content_key("新的词还在增加。"));
}

#[test]
#[cfg(target_os = "windows")]
fn admitted_prefix_planner_rejects_fresh_rewrites_and_new_authority() {
    for case in 0..4 {
        let (asr,anchor)=admitted_tail_snapshot_fixture();
        grow_admitted_tail(&asr,Instant::now());
        let candidate=asr.selected_live_owner_candidate().unwrap();
        {
            let mut state=asr.state.lock();
            match case {
                0=>{state.pause_early_recent_revisions.clear();},
                1=>state.pause_early_stability_floor=Some(Instant::now()),
                2=>state.pause_early_recent_revisions.front_mut().unwrap().1="旧的另一段正文。".into(),
                _=>state.pause_early_stability_source=Some(OwnerDeliveryStabilitySource {
                    decoder:candidate.stability_source.decoder+1,..candidate.stability_source}),
            }
        }
        assert!(admitted_snapshot_stable_end(&mut asr.state.lock(),&candidate,&anchor,Instant::now()).is_none(),"case={case}");
    }
}

#[test]
fn source_snapshot_extracts_admitted_native_rendering_only_with_its_own_word_alignment() {
    let asr=uplink_stall_test_asr();
    let response=json!({"text":"我先整理文件。明天再检查设备。","utterances":[
        {"text":"我先整理文件。","definite":true,"additions":{"source":"two_pass"},
            "words":[{"text":"我先整理文件。","start_time":100,"end_time":900}]},
        {"text":"明天在检查设备。","definite":true,"additions":{"source":"two_pass"},
            "words":[{"text":"明天再检查设备。","start_time":1200,"end_time":2200}]}]});
    let mut state=asr.state.lock();
    state.recognition_boundary.update(&response);
    state.provider_timed_transcript=Some(super::super::volcengine_transcript::timed_transcript_from_result(&response));
    assert_eq!(source_snapshot_confirmation(&state,false).unwrap().text,"我先整理文件。",
        "whole rendering is no source-projected word-ownership grant");
    assert_eq!(source_snapshot_confirmation(&state,true).unwrap().text,"我先整理文件。明天再检查设备。");
    state.provider_timed_transcript.as_mut().unwrap().words[1].text="明天在检查设备。".into();
    assert_eq!(source_snapshot_confirmation(&state,true).unwrap_err(),"confirmed_rendering_not_timed",
        "a native string cannot borrow another rendering's word coordinates");
}

#[test]
#[cfg(target_os = "windows")]
fn synthetic_provider_frames_confirm_a_tail_while_later_primary_words_are_growing() {
    use crate::transcript_boundary::{content_key,TimedTranscriptWord};
    let frames:Vec<Value>=serde_json::from_str(include_str!("testdata/source_snapshot_synthetic_frames.json")).unwrap();
    let child=uplink_stall_test_asr().with_owner_activity_only(true);
    for frame in &frames {child.apply_saved_native_result_for_test(&frame["result"],frame["final"].as_bool().unwrap());}
    let confirmed=source_snapshot_confirmation(&child.state.lock(),true).unwrap();
    let opening=frames.last().unwrap()["result"]["utterances"][0]["text"].as_str().unwrap();
    let boundary=confirmed.delivery_boundary(&content_key(opening)).unwrap();
    let asr=uplink_stall_test_asr().with_owner_activity_only(true);
    let source=LiveDeliverySource {capture_stream_id:asr.asr_stream_id(),
        capture_generation:asr.source_snapshot.lock().capture_generation,
        decoder_stream_id:asr.asr_stream_id(),owner_generation:asr.state.lock().owner_delivery_generation};
    let anchor=LiveLedgerAnchor {source,generation:1,source_text:opening.into(),source_key:content_key(opening),boundary};
    {
        let mut state=asr.state.lock();
        let mut growing=confirmed.clone();growing.text.push_str("新的词还在增加。");
        growing.words.push(TimedTranscriptWord {text:"新的词还在增加。".into(),start_ms:Some(18_000),end_ms:Some(19_500)});
        state.is_connected=true;state.next_input_destination_offset=20_000*32;
        state.best_transcript_text=growing.text.clone();state.best_transcript_timing=Some(growing.clone());
        state.provider_timed_transcript=Some(growing);state.best_transcript_committed_at=Some(Instant::now());
        state.recognition_boundary.update(&frames.last().unwrap()["result"]);
    }
    *asr.retained_pcm.lock()=vec![0;20_000*32];
    {let mut slot=asr.source_snapshot.lock();slot.closed=false;slot.parent_stream_id=Some(asr.asr_stream_id());
        slot.result=Some(SourceSnapshotResult {anchor:Some(anchor.clone()),parent_stream_id:asr.asr_stream_id(),
            parent_lifecycle_generation:slot.lifecycle_generation,source_epoch:source.owner_generation,child_decoder_id:child.asr_stream_id(),
            captured_origin_bytes:0,captured_pcm_bytes:20_000*32,captured_through_ms:20_000,confirmed:confirmed.clone(),completed_at:Instant::now()});}
    let continued=asr.pause_early_ledger_continuation_after(Duration::from_secs(1),&anchor).unwrap();
    assert_eq!(continued.source_text(),confirmed.text,"the actual synthetic provider rendering must append once before capture ends");
    assert!(!continued.owner_caught_up);
    assert!(asr.ledger_continuation_is_current(&continued));
    let next=continued.advance(&content_key(&continued.suffix.timing.text),&content_key(&continued.source_text())).unwrap();
    assert_eq!(asr.pending_body_delivery_after(&next.source_key).unwrap().key,content_key("新的词还在增加。"));
}

#[test]
fn snapshot_omitted_context_requires_first_new_audio_occurrence_not_a_later_repeat() {
    use crate::transcript_boundary::{content_key,TimedTranscript,TimedTranscriptWord};
    let parent=TimedTranscript {text:"已上屏。继续继续。".into(),words:vec![
        TimedTranscriptWord {text:"已上屏。".into(),start_ms:Some(100),end_ms:Some(500)},
        TimedTranscriptWord {text:"继续".into(),start_ms:Some(1000),end_ms:Some(1300)},
        TimedTranscriptWord {text:"继续。".into(),start_ms:Some(1600),end_ms:Some(1900)}]};
    let submitted=parent.delivery_boundary(&content_key("已上屏。")).unwrap();
    let pending=parent.timed_suffix_after_delivery(&submitted).unwrap();
    let child=TimedTranscript {text:"继续继续。".into(),words:parent.words[1..].to_vec()};
    assert!(child.timed_suffix_after_delivery(&submitted).is_none());
    assert!(snapshot_child_suffix(&child,&pending,&submitted,true).is_some());
    assert!(snapshot_child_suffix(&child,&pending,&submitted,false).is_none(),"source-projected ownership retains its existing seam");
    let later=TimedTranscript {text:"继续。".into(),words:vec![parent.words[2].clone()]};
    assert!(snapshot_child_suffix(&later,&pending,&submitted,true).is_none(),"a matching later repetition cannot skip the first unsubmitted word");
    let mut unknown=child.clone();unknown.words[0].start_ms=None;
    assert!(snapshot_child_suffix(&unknown,&pending,&submitted,true).is_none());
    let mut overlapping=child.clone();overlapping.words[0].start_ms=Some(400);
    assert!(snapshot_child_suffix(&overlapping,&pending,&submitted,true).is_none(),"new-only path cannot repeat an already submitted interval");
}

#[test]
fn captured_synthetic_child_can_omit_old_context_without_blocking_new_words() {
    use crate::transcript_boundary::content_key;
    let frames:Vec<Value>=serde_json::from_str(include_str!("testdata/source_snapshot_synthetic_frames.json")).unwrap();
    let parent=super::super::volcengine_transcript::timed_transcript_from_result(&frames.last().unwrap()["result"]);
    let opening=frames.last().unwrap()["result"]["utterances"][0]["text"].as_str().unwrap();
    let submitted=parent.delivery_boundary(&content_key(opening)).unwrap();
    let pending=parent.timed_suffix_after_delivery(&submitted).unwrap();
    let saved:Value=serde_json::from_str(include_str!("testdata/source_snapshot_synthetic_no_context.json")).unwrap();
    let decoder=uplink_stall_test_asr().with_owner_activity_only(true);
    decoder.apply_saved_native_result_for_test(&saved["result"],true);
    let confirmed=source_snapshot_confirmation(&decoder.state.lock(),true).unwrap();
    let child=confirmed.shifted_from_clip(saved["originMs"].as_u64().unwrap(),saved["throughMs"].as_u64().unwrap()).unwrap();
    assert!(child.timed_suffix_after_delivery(&submitted).is_none(),"saved failure: the decoder did not repeat the old context");
    let suffix=snapshot_child_suffix(&child,&pending,&submitted,true).unwrap();
    let common=crate::transcript_boundary::stable_content_prefix(&suffix.text,std::iter::once(pending.text.as_str()));
    let safe=snapshot_timed_prefix(&suffix,&content_key(&common)).unwrap();
    assert_eq!(safe.text,"前半句已经进入");
    assert!(!safe.text.contains('书'),"the child correction that disagrees with the parent remains pending");
}

#[tokio::test]
#[ignore = "one explicit synthetic PCM request to existing configured ASR; no microphone/editor/profile mutation"]
async fn diagnostic_synthetic_source_snapshot_decoder_response() {
    let root=std::path::PathBuf::from(std::env::var_os("LISTENER_SOURCE_SNAPSHOT_DIAG_DIR").expect("explicit synthetic diagnostic directory"));
    let wav=std::fs::read(root.join("synthetic_tail.wav")).unwrap();
    let mut pcm=denzic_audio_v1_core::read_wav_pcm16le(&wav).unwrap();
    assert!(pcm.len()<30*32000 && !pcm.is_empty());
    pcm.extend(vec![0;32000]);
    let proxy=crate::coordinator::read_asr_proxy_config("volcengine").unwrap();
    let asr=Arc::new(VolcengineStreamingASR::new_with_proxy_config(
        crate::coordinator::read_volc_credentials(),Vec::new(),proxy).with_owner_activity_only(true));
    let _cancel=CancelSnapshotDecoder(Arc::clone(&asr));
    let began=Instant::now();
    let result=tokio::time::timeout(Duration::from_secs(30),async {
        asr.open_session().await?;
        asr.mark_audio_delivery_ready();
        AudioConsumer::consume_pcm_chunk(&*asr,&pcm);
        asr.send_last_frame().await?;
        asr.await_final_result().await
    }).await;
    assert!(matches!(&result,Ok(Ok(_))),"configured synthetic request failed; no live capture or user data was used");
    let state=asr.state.lock();
    let timing=state.provider_timed_transcript.as_ref();
    let strict=timing.and_then(|t|state.recognition_boundary.confirmed_prefix(&t.text,Some(t)));
    let admitted=timing.and_then(|t|state.recognition_boundary.confirmed_admitted_prefix(&t.text,Some(t)));
    let strict_timed=timing.zip(strict.as_ref()).and_then(|(t,p)|snapshot_timed_prefix(t,&crate::transcript_boundary::content_key(p)));
    let admitted_timed=timing.zip(admitted.as_ref()).and_then(|(t,p)|snapshot_timed_prefix(t,&crate::transcript_boundary::content_key(p)));
    let numeric=serde_json::json!({"synthetic":true,"requestCount":1,"elapsedMs":began.elapsed().as_millis(),
        "providerTimingChars":timing.map(|t|t.text.chars().count()),"providerTimingWords":timing.map(|t|t.words.len()),
        "requiresConfirmation":state.recognition_boundary.requires_confirmation(),
        "strictConfirmedChars":strict.as_ref().map(|s|s.chars().count()),"admittedConfirmedChars":admitted.as_ref().map(|s|s.chars().count()),
        "strictTimedCertificate":strict_timed.is_some(),"admittedTimedCertificate":admitted_timed.is_some(),
        "syntheticFinalChars":state.best_transcript_text.chars().count()});
    std::fs::write(root.join("synthetic-child-response-numeric.json"),serde_json::to_vec_pretty(&numeric).unwrap()).unwrap();
    // Every frame is generated from the explicitly synthetic WAV above. Keep
    // the actual provider shape for deterministic parser/worker regression.
    std::fs::write(root.join("synthetic-provider-frames.json"),serde_json::to_vec_pretty(&state.diagnostic_provider_responses).unwrap()).unwrap();
    println!("{numeric}");
}

#[tokio::test]
#[ignore = "explicit real-time synthetic ASR/tail worker integration; existing service usage, no microphone/editor/profile writes"]
async fn diagnostic_synthetic_continuous_tail_pipeline() {
    use crate::transcript_boundary::content_key;
    let root=std::path::PathBuf::from(std::env::var_os("LISTENER_SYNTHETIC_TAIL_PIPELINE_DIAG_DIR").unwrap());
    let wav=std::fs::read(root.join("full-synthetic-baseline-synthetic_tail.wav")).unwrap();
    let mut pcm=denzic_audio_v1_core::read_wav_pcm16le(&wav).unwrap();
    let speech_ms=pcm.len()/32;assert!(speech_ms<30_000);
    pcm.extend(vec![0;32_000]);
    let asr=Arc::new(VolcengineStreamingASR::new_with_proxy_config(crate::coordinator::read_volc_credentials(),
        Vec::new(),crate::coordinator::read_asr_proxy_config("volcengine").unwrap()).with_owner_activity_only(true));
    let _cancel=CancelSnapshotDecoder(Arc::clone(&asr));
    asr.open_session().await.unwrap();asr.mark_audio_delivery_ready();
    let start=Instant::now();let mut sent_ms=0;let mut consumed=String::new();
    let mut anchor:Option<LiveLedgerAnchor>=None;let mut events=Vec::new();
    for chunk in pcm.chunks(3200) {
        AudioConsumer::consume_pcm_chunk(&*asr,chunk);sent_ms+=chunk.len()/32;
        tokio::time::sleep(Duration::from_millis((chunk.len()/32)as u64)).await;
        if let Some(continued)=anchor.as_ref().and_then(|a|asr.pause_early_ledger_continuation_after(Duration::from_secs(1),a)) {
            let text=continued.source_text();let key=content_key(&text);
            if key.starts_with(&consumed)&&key.len()>consumed.len() {
                if let Some(next)=continued.advance(&content_key(&continued.suffix.timing.text),&key) {
                    let parent_age=asr.selected_live_owner_candidate().map(|c|c.updated_at.elapsed().as_millis());
                    consumed=key;anchor=Some(next);
                    events.push(json!({"sentMs":sent_ms,"elapsedMs":start.elapsed().as_millis(),"source":"child","chars":text.chars().count(),"parentAgeMs":parent_age}));
                    continue;
                }
            }
        }
        if let Some(view)=asr.pause_early_delivery_evidence_after(Duration::from_secs(1),&consumed) {
            let key=content_key(&view.text);
            if key.starts_with(&consumed)&&key.len()>consumed.len() {
                if let Some(boundary)=view.timing.as_ref().and_then(|t|t.timing.delivery_boundary(&key)) {
                    let generation=anchor.as_ref().map_or(1,|a|a.generation+1);
                    anchor=Some(LiveLedgerAnchor {source:view.source,generation,source_key:key.clone(),source_text:view.text.clone(),boundary});
                    consumed=key;events.push(json!({"sentMs":sent_ms,"elapsedMs":start.elapsed().as_millis(),"source":"primary","chars":view.text.chars().count()}));
                }
            }
        }
    }
    let before_end=events.clone();
    asr.send_last_frame().await.unwrap();
    let final_result=asr.await_final_result().await.unwrap();
    let slot=asr.source_snapshot.lock();
    let data=json!({"synthetic":true,"speechMs":speech_ms,"sentMs":sent_ms,"beforeProtocolEnd":before_end,
        "earlyChildWhileParentGrowing":events.iter().any(|e|e["source"]=="child"&&e["sentMs"].as_u64().is_some_and(|t|t<(speech_ms as u64))&&e["parentAgeMs"].as_u64().is_some_and(|age|age<1000)),
        "requests":slot.next_request_id,"replayedPcmBytes":slot.replayed_pcm_bytes,"inputBytes":pcm.len(),
        "finalChars":final_result.text.chars().count(),"lastSnapshotOutcome":slot.last_outcome});
    std::fs::write(root.join("synthetic-continuous-pipeline.json"),serde_json::to_vec_pretty(&data).unwrap()).unwrap();
    println!("{data}");
}

