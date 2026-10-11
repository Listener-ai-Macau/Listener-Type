//! SAUC 1:1 cloud identity. Only a registered name grants owner output.
//! Anonymous diarization, missing labels and local wake matches are not cloud identity.
use super::*;
use super::super::cloud_voiceprint::{CloudOwnerVoiceprint, CloudVoiceprintClient};

struct CloudOwnerProjection {
    accepted: Value,
    preview: Value,
    pending: String,
}

fn project(result: &Value, name: &str, has_final: bool) -> CloudOwnerProjection {
    let mut accepted = Vec::new();
    let mut preview = Vec::new();
    let mut pending = String::new();
    let mut row_text = String::new();
    if let Some(rows) = result.get("utterances").and_then(Value::as_array) {
        for row in rows {
            row_text.push_str(row.get("text").and_then(Value::as_str).unwrap_or_default());
            if utterance_speaker_id(row).as_deref() == Some(name) {
                preview.push(row.clone());
                if has_final || utterance_is_stable(row) { accepted.push(row.clone()); }
                else { pending.push_str(row.get("text").and_then(Value::as_str).unwrap_or_default()); }
            } else {
                // A non-match is withheld. An anonymous ID alone does not prove Other.
                pending.push_str(row.get("text").and_then(Value::as_str).unwrap_or_default());
            }
        }
    } else {
        pending = result.get("text").and_then(Value::as_str).unwrap_or_default().to_string();
    }
    let raw_text = result.get("text").and_then(Value::as_str).unwrap_or_default();
    if crate::transcript_boundary::content_key(raw_text) != crate::transcript_boundary::content_key(&row_text) {
        // Text outside attributed rows remains unresolved, even if preceding rows matched.
        pending = raw_text.to_string();
    }
    let view = |rows: Vec<Value>| {
        let text: String = rows.iter().filter_map(|row| row.get("text").and_then(Value::as_str)).collect();
        let mut value = result.clone();
        value["text"] = Value::String(text);
        value["utterances"] = Value::Array(rows);
        value
    };
    CloudOwnerProjection { accepted: view(accepted), preview: view(preview), pending }
}

impl VolcengineStreamingASR {
    pub fn with_cloud_owner(mut self, profile: Option<CloudOwnerVoiceprint>) -> Self {
        *self.cloud_owner.get_mut() = profile;
        if self.cloud_owner.get_mut().is_some() {
            self.session_options.endpoint = VolcengineSessionEndpoint::OptimizedBidirectional;
            self.session_options.enable_nonstream = true;
            self.session_options.result_type = VolcengineResultType::Full;
            self.state.get_mut().target_speaker_id = self.cloud_owner.get_mut().as_ref().map(|owner| owner.speaker_name.clone());
        }
        self
    }

    pub fn with_managed_cloud(mut self, enabled: bool) -> Self {
        self.managed_cloud = enabled;
        self.managed_account_epoch.store(super::super::cloud_gateway::account_epoch(),Ordering::Release);
        if enabled {
            *self.cloud_owner.get_mut() = None;
            self.session_options.endpoint = VolcengineSessionEndpoint::OptimizedBidirectional;
            self.session_options.enable_nonstream = true;
            self.session_options.result_type = VolcengineResultType::Full;
        }
        self
    }

    pub(super) fn managed_account_is_current(&self) -> bool {
        self.managed_account_epoch.load(Ordering::Acquire)==super::super::cloud_gateway::account_epoch()
    }

    pub fn cloud_owner_enabled(&self) -> bool { self.managed_cloud || self.cloud_owner.lock().is_some() }

    pub(super) async fn verify_cloud_owner_configuration(&self) -> Result<(), VolcengineASRError> {
        let Some(profile) = self.cloud_owner.lock().clone() else { return Ok(()); };
        profile.validate_for(&self.credentials).map_err(VolcengineASRError::CloudOwnerUnavailable)?;
        let client = CloudVoiceprintClient::new(self.credentials.clone(), &self.proxy_config)
            .map_err(VolcengineASRError::CloudOwnerUnavailable)?;
        let actual = client.select(&profile.id).await.map_err(VolcengineASRError::CloudOwnerUnavailable)?;
        if actual != profile {
            return Err(VolcengineASRError::CloudOwnerUnavailable("云端本人声纹已修改，请在设置里重新选择".into()));
        }
        Ok(())
    }

    pub(super) fn process_cloud_owner_result(&self, result: &Value, has_final: bool, trace_frame: usize) -> bool {
        if *self.last_final_seal_authority_label.lock() == Some("cloud_owner") { return false; }
        let Some(profile) = self.cloud_owner.lock().clone() else {
            self.state.lock().pending_unattributed_text=result.get("text").and_then(Value::as_str).unwrap_or_default().to_string();
            return true;
        };
        let projection = project(result, &profile.speaker_name, has_final);
        let text = projection.accepted.get("text").and_then(Value::as_str).unwrap_or_default().to_string();
        let visual = projection.preview.get("text").and_then(Value::as_str).unwrap_or_default().to_string();
        let timing = super::super::volcengine_transcript::timed_transcript_from_result(&projection.accepted);
        let end_ms = projection.accepted.get("utterances").and_then(Value::as_array)
            .into_iter().flatten().filter_map(utterance_end_ms).max();
        let (update, changed, visual_changed, commit) = {
            let mut state = self.state.lock();
            let previous_end = state.target_speech_end_ms;
            state.target_speaker_id = Some(profile.speaker_name.clone());
            state.speaker_info_present |= result.get("utterances").and_then(Value::as_array)
                .is_some_and(|rows| rows.iter().any(|row| utterance_speaker_id(row).is_some()));
            state.target_speech_end_ms = previous_end.into_iter().chain(end_ms).max();
            state.qualified_owner_speech_end_ms = state.target_speech_end_ms;
            state.stable_attributed_speech_end_ms = state.target_speech_end_ms;
            let advanced = state.target_speech_end_ms != previous_end;
            let pending_changed = state.pending_unattributed_text != projection.pending;
            state.pending_unattributed_text = projection.pending;
            // Full-result mode is one authoritative revision. Do not merge an
            // older unverified row or a raw-provider tail into this projection.
            let changed = state.last_partial_text != text;
            if changed { state.best_transcript_committed_at = Some(Instant::now()); }
            state.best_transcript_text = text.clone();
            state.last_partial_text = text.clone();
            state.best_transcript_timing = Some(timing.clone());
            state.speaker_filtered_timed_transcript = Some(timing.clone());
            state.best_transcript_segments = transcript_candidate_from_result(&projection.accepted).timed_segments;
            state.best_untimed_window.clear();
            state.optimistic_preview_text = visual.clone();
            let visual_changed = state.last_emitted_visual_preview_text != visual;
            state.last_emitted_visual_preview_text = visual.clone();
            state.verified_provider_owner = (!text.is_empty()).then(|| RawTranscript {
                text: text.clone(), duration_ms: end_ms.unwrap_or_default(),
            });
            state.verified_provider_owner_timing = (!text.is_empty()).then(|| timing.clone());
            state.verified_provider_owner_is_final = has_final && !text.is_empty();
            if changed { state.verified_provider_owner_updated_at = Some(Instant::now()); }
            let update = target_speaker_update_from_state(&state, advanced, pending_changed, advanced);
            let commit = if has_final { state.transcript_evidence.commit_once(&text, false) } else { None };
            (update, changed, visual_changed, commit)
        };
        self.record_diagnostic_trace(trace_frame, has_final, "cloud_owner_projection", &text);
        self.emit_target_speaker_update(update);
        if visual_changed {
            let callback = self.visual_partial_callback.lock().clone();
            if let Some(callback) = callback { callback(VisualTranscriptUpdate::Provisional(visual)); }
        }
        if (changed || has_final) && !text.is_empty() {
            self.state.lock().last_emitted_preview_text = text.clone();
            self.emit_partial_transcript(&text);
            if !has_final { self.emit_streaming_event(VolcengineStreamingEvent::Partial(text.clone())); }
        }
        if has_final {
            *self.last_final_seal_authority_label.lock() = Some("cloud_owner");
            if let Some(commit) = commit {
                self.emit_streaming_event(VolcengineStreamingEvent::Final(commit.text.clone()));
                self.signal_success(RawTranscript { text: commit.text, duration_ms: end_ms.unwrap_or_default() });
            }
            self.state.lock().is_connected = false;
            *self.audio_tx.lock() = None;
            return false;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(text: &str, speaker: Value, end: u64, stable: bool) -> Value {
        json!({"text":text,"definite":stable,"start_time":0,"end_time":end,"additions":{"speaker":speaker},
            "words":[{"text":text,"start_time":0,"end_time":end}]})
    }

    fn asr() -> VolcengineStreamingASR {
        VolcengineStreamingASR::new(VolcengineCredentials {
            app_id:"test-app".into(),access_token:"test-token".into(),resource_id:"test-resource".into(),
        },Vec::new()).with_cloud_owner(Some(CloudOwnerVoiceprint {
            id:"a3ee18be-6113-4b9a-8bc1-55fbb9f88f71".into(),speaker_name:"Billy".into(),account_fingerprint:"test".into(),
        }))
    }

    fn send(asr: &VolcengineStreamingASR, result: Value, final_packet: bool) -> bool {
        let payload = serde_json::to_vec(&json!({"result":result,"audio_info":{"duration":1000}})).unwrap();
        asr.handle_frame(&frame::build(MessageType::FullServerResponse,
            if final_packet { Flags::LastPacket } else { Flags::None }, Serialization::Json,&payload,None))
    }

    #[test]
    fn cloud_owner_recorded_frames_preserve_authorized_words_across_all_output_views() {
        let fixture: Value = serde_json::from_str(include_str!("volcengine_cloud_owner_recorded_fixture.json")).unwrap();
        for trial in fixture["trials"].as_array().unwrap() {
            let cloud = asr();
            let partials = Arc::new(ParkingMutex::new(Vec::new()));
            let sink = Arc::clone(&partials);
            cloud.set_partial_transcript_callback(Some(Arc::new(move |text| sink.lock().push(text))));
            let (tx, mut rx) = oneshot::channel();
            cloud.state.lock().final_tx = Some(tx);
            for frame in trial["frames"].as_array().unwrap() {
                let final_packet = frame["final"].as_bool().unwrap();
                let expected = frame["expectedOwner"].as_str().unwrap();
                let expected_preview = frame["expectedPreview"].as_str().unwrap();
                send(&cloud, frame["result"].clone(), final_packet);
                {
                    let state = cloud.state.lock();
                    assert_eq!(state.best_transcript_text, expected, "{} at {} ms", trial["label"], frame["observedAtMs"]);
                    assert_eq!(state.optimistic_preview_text, expected_preview);
                    assert_eq!(state.best_transcript_timing.as_ref().unwrap().text, expected);
                    // Anonymous rows stay unresolved; withholding text does not
                    // turn absent identity into confirmed Other or silence.
                    assert!(state.local_non_target_speech_end_ms.is_none());
                }
                if expected.is_empty() {
                    assert!(cloud.selected_live_owner_candidate().is_none());
                    assert!(cloud.primary_output_text(frame["result"]["text"].as_str().unwrap()).is_none());
                } else {
                    assert_eq!(cloud.selected_live_owner_candidate().unwrap().text, expected);
                    assert_eq!(cloud.primary_output_text(expected).as_deref(), Some(expected));
                    assert_eq!(partials.lock().last().map(String::as_str), Some(expected));
                }
            }
            let expected = trial["expectedFinal"].as_str().unwrap();
            assert_eq!(rx.try_recv().unwrap().unwrap().text, expected);
            match cloud.verified_provider_owner_final_snapshot() {
                Some((text, timing)) => {
                    assert!(!expected.is_empty());
                    assert_eq!(text.text, expected);
                    assert_eq!(timing.text, expected);
                }
                None => assert!(expected.is_empty()),
            }
        }
    }

    #[test]
    fn cloud_owner_request_is_one_registered_identity_and_legacy_request_is_unchanged() {
        let cloud = asr();
        let request = cloud.build_first_frame_payload("test");
        assert_eq!(request["request"]["ssd_mode"],2);
        assert_eq!(request["request"]["voiceprints"],json!([{"id":"a3ee18be-6113-4b9a-8bc1-55fbb9f88f71"}]));
        assert_eq!(request["request"]["enable_nonstream"],true);
        let legacy = VolcengineStreamingASR::new(cloud.credentials.clone(),Vec::new());
        let legacy_request = legacy.build_first_frame_payload("test");
        assert!(legacy_request["request"].get("ssd_mode").is_none());
        assert!(legacy_request["request"].get("voiceprints").is_none());
        assert!(!legacy.target_speaker_filter_was_required());
    }

    #[test]
    fn cloud_owner_frames_use_one_policy_for_callbacks_timing_and_final_delivery() {
        let cloud = asr();
        let partials = Arc::new(ParkingMutex::new(Vec::new()));
        let sink = Arc::clone(&partials);
        cloud.set_partial_transcript_callback(Some(Arc::new(move |text| sink.lock().push(text))));
        let (tx,mut rx) = oneshot::channel();
        cloud.state.lock().final_tx = Some(tx);
        let mixed = json!({"text":"媒体今旁人", "utterances":[row("媒体",json!(0),100,true),row("今",json!("Billy"),200,true),row("旁人",json!(1),900,true)]});
        assert!(send(&cloud,mixed.clone(),false));
        assert_eq!(cloud.state.lock().best_transcript_text,"今");
        assert_eq!(cloud.state.lock().target_speech_end_ms,Some(200));
        assert_eq!(cloud.state.lock().pending_unattributed_text,"媒体旁人");
        assert!(cloud.target_speaker_filter_was_required());
        assert_eq!(cloud.selected_live_owner_candidate().unwrap().text,"今");
        assert!(!send(&cloud,mixed,true));
        assert_eq!(rx.try_recv().unwrap().unwrap().text,"今");
        let (final_text,timing) = cloud.verified_provider_owner_final_snapshot().unwrap();
        assert_eq!(final_text.text,"今");
        assert_eq!(timing.text,"今");
        assert!(partials.lock().iter().all(|text| text == "今"));
    }

    #[test]
    fn cloud_owner_ignores_local_identity_veto_and_does_not_start_a_separator() {
        let cloud = Arc::new(asr());
        cloud.note_verified_local_speaker_tracking_started("开始录音");
        cloud.note_local_speaker_observation(1000,
            crate::speaker_verification::SessionSpeakerClassification::NonTarget { score:0.0 },
            crate::speech_decision_kernel::TranscriptSpeakerEvidence::HardNonTarget);
        assert!(!cloud.state.lock().local_speaker_tracking_enabled);
        assert!(cloud.state.lock().local_non_target_speech_end_ms.is_none());
        assert!(!cloud.may_run_local_shadow_decode());
        #[cfg(all(target_os = "windows", feature = "target-speaker-extraction"))]
        {
            cloud.start_target_speaker_extraction("开始录音",&[],0.0);
            assert!(cloud.target_speaker_stream.lock().is_none());
        }
        assert!(send(&cloud,json!({"text":"轻声", "utterances":[row("轻声",json!("Billy"),1200,true)]}),false));
        assert_eq!(cloud.state.lock().best_transcript_text,"轻声");
    }

    #[test]
    fn cloud_owner_final_missing_identity_cannot_restore_an_old_preview_or_raw_fallback() {
        let cloud = asr();
        let (tx,mut rx) = oneshot::channel();
        cloud.state.lock().final_tx = Some(tx);
        send(&cloud,json!({"text":"待定", "utterances":[row("待定",json!("Billy"),200,false)]}),false);
        assert!(cloud.state.lock().best_transcript_text.is_empty());
        assert!(!send(&cloud,json!({"text":"短剧","utterances":[row("短剧",json!(0),900,true)]}),true));
        assert_eq!(rx.try_recv().unwrap().unwrap().text,"");
        assert!(cloud.verified_provider_owner_final_snapshot().is_none());
        let committed = cloud.state.lock().best_transcript_text.clone();
        assert!(!send(&cloud,json!({"text":"再次本人", "utterances":[row("再次本人",json!("Billy"),1000,true)]}),true));
        assert_eq!(cloud.state.lock().best_transcript_text,committed);
    }

    #[test]
    fn cloud_owner_disconnect_never_commits_pending_or_anonymous_raw_text() {
        let cloud = asr();
        let (tx,mut rx) = oneshot::channel();
        cloud.state.lock().final_tx = Some(tx);
        send(&cloud,json!({"text":"媒体", "utterances":[row("媒体",json!(0),900,true)]}),false);
        cloud.fallback_to_partial_or_error(VolcengineASRError::NoFinalResult);
        assert!(rx.try_recv().unwrap().is_err());
        let owner = asr();
        let (tx,mut rx) = oneshot::channel();
        owner.state.lock().final_tx = Some(tx);
        send(&owner,json!({"text":"本人尾", "utterances":[row("本人",json!("Billy"),200,true),row("尾",json!("Billy"),300,false)]}),false);
        owner.fallback_to_partial_or_error(VolcengineASRError::NoFinalResult);
        assert_eq!(rx.try_recv().unwrap().unwrap().text,"本人");
    }

    #[test]
    fn cloud_owner_unattributed_raw_growth_does_not_advance_the_owner_clock() {
        let cloud = asr();
        let owner = row("已",json!("Billy"),200,true);
        send(&cloud,json!({"text":"已", "utterances":[owner.clone()]}),false);
        send(&cloud,json!({"text":"已后面的声音", "utterances":[owner]}),false);
        assert_eq!(cloud.state.lock().best_transcript_text,"已");
        assert_eq!(cloud.state.lock().target_speech_end_ms,Some(200));
        assert!(!cloud.state.lock().pending_unattributed_text.is_empty());
    }

    #[test]
    fn cloud_owner_does_not_bind_the_first_anonymous_speaker_or_raw_tail() {
        let result = json!({"text":"短剧今旁人", "utterances":[row("短剧",json!(0),100,true),row("今",json!("Billy"),200,true),row("旁人",json!(1),300,true)]});
        let projected = project(&result,"Billy",false);
        assert_eq!(projected.accepted["text"],"今");
        assert_eq!(projected.preview["text"],"今");
        assert_eq!(projected.pending,"短剧旁人");
        assert_eq!(projected.accepted["utterances"][0]["words"],result["utterances"][1]["words"]);
    }

    #[test]
    fn cloud_owner_pending_labels_cannot_be_committed_even_at_protocol_final() {
        for result in [json!({"text":"短剧"}),json!({"text":"短剧","utterances":[row("短剧",Value::Null,400,true)]})] {
            let projected = project(&result,"Billy",true);
            assert_eq!(projected.accepted["text"],"");
            assert_eq!(projected.pending,"短剧");
        }
        let pending = json!({"text":"尾字","utterances":[row("尾字",json!("Billy"),500,false)]});
        assert_eq!(project(&pending,"Billy",false).accepted["text"],"");
        assert_eq!(project(&pending,"Billy",false).preview["text"],"尾字");
        assert_eq!(project(&pending,"Billy",true).accepted["text"],"尾字");
    }

    #[test]
    fn cloud_managed_pending_configuration_never_grants_an_owner_row() {
        let cloud=asr().with_managed_cloud(true);
        assert!(cloud.cloud_owner_enabled());
        send(&cloud,json!({"text":"今","utterances":[row("今",json!("Billy"),200,true)]}),false);
        assert_eq!(cloud.state.lock().best_transcript_text,"");
        assert_eq!(cloud.state.lock().pending_unattributed_text,"今");
        assert_eq!(cloud.state.lock().target_speech_end_ms,None);
    }

    #[tokio::test]
    async fn cloud_managed_changed_account_stops_before_opening_or_accepting_frames() {
        let cloud=std::sync::Arc::new(asr().with_managed_cloud(true));
        cloud.managed_account_epoch.store(super::super::super::cloud_gateway::account_epoch().wrapping_sub(1),Ordering::Release);
        assert!(!cloud.managed_account_is_current());
        assert!(matches!(cloud.open_session_inner().await,Err(VolcengineASRError::CloudOwnerUnavailable(_))));
        send(&cloud,json!({"text":"旧账号","utterances":[row("旧账号",json!("Billy"),200,true)]}),false);
        assert_eq!(cloud.state.lock().best_transcript_text,"");
    }
}
