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

