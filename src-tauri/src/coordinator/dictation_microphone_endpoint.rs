// Native microphone capture uses the existing endpoint evidence and normal
// finalization. It has no BLE segment, firmware STOP or synthetic BLE stats.

fn arm_microphone_wake_body_guard(inner: &Arc<Inner>, session: SessionId) {
    // The native handoff already removes the activation PCM at its proven
    // boundary. Reuse the automatic body lifecycle with no textual prefix to
    // strip: a literal wake phrase spoken later is ordinary body content.
    arm_automatic_wake_text_guard(inner, session, String::new(), 0);
}

struct MicrophoneEndpointConsumer {
    target: Arc<dyn crate::recorder::AudioConsumer>,
    asr: Arc<VolcengineStreamingASR>,
    capture: Mutex<(LocalSpeechActivity, u64)>,
}

impl crate::recorder::AudioConsumer for MicrophoneEndpointConsumer {
    fn consume_pcm_chunk(&self, pcm: &[u8]) {
        if pcm.is_empty() || pcm.len() % 2 != 0 { return; }
        let (end, evidence) = {
            let mut capture = self.capture.lock();
            let start = capture.1;
            let end = start + (pcm.len() / 2) as u64;
            let evidence = capture.0.submit(start, end, pcm);
            capture.1 = end;
            (end, evidence)
        };
        let (rms, peak) = embedded_pcm_rms_and_peak(pcm);
        let energy = embedded_streaming_chunk_has_speech_energy(rms, peak);
        let speech_end = embedded_confirmed_speech_end_ms(energy, evidence, end);
        self.asr.note_local_audio_activity_with_speech_end_samples(end / 16, end, speech_end);
        self.asr.note_local_speech_activity(evidence);
        self.asr.submit_personal_vad(pcm, end);
        self.target.consume_pcm_chunk(pcm);
    }
}

fn microphone_endpoint_consumer(
    inner: &Arc<Inner>, session: SessionId, asr: &Arc<VolcengineStreamingASR>,
    target: Arc<dyn crate::recorder::AudioConsumer>,
) -> Arc<dyn crate::recorder::AudioConsumer> {
    if !inner.microphone_wake.owns_dictation_session(session) { return target; }
    let activity = LocalSpeechActivity::with_quality_timeline(
        asr.local_speech_activity_sink(), asr.local_speech_quality_sink());
    Arc::new(MicrophoneEndpointConsumer { target, asr: Arc::clone(asr), capture: Mutex::new((activity, 0)) })
}

fn begin_microphone_endpoint_stop(
    inner: &Arc<Inner>, admission: &EndpointStopAdmission,
) -> Option<DictationTransition> {
    let session = admission.ticket.session_id;
    if !inner.microphone_wake.owns_dictation_session(session) || !admission.is_valid(inner)
        || !microphone_endpoint_silence_qualified(admission) { return None; }
    if !admission.stop_state.lock().begin_sending(admission.ticket) || !admission.is_valid(inner)
        || !microphone_endpoint_silence_qualified(admission) { return None; }
    let transition = {
        let mut state = inner.state.lock();
        // Keep the phase, session and native-recorder owner in one admission
        // boundary. An old proposal cannot stop a replacement recording.
        if state.session_id != session || state.cancelled || state.phase != SessionPhase::Listening
            || !inner.recorder.lock().as_ref().is_some_and(|capture| capture.session_id == session) { return None; }
        apply_dictation_event(&mut state, DictationEvent::Stop { session_id: session, user_initiated: false })
    };
    if !matches!(transition, DictationTransition::Applied { .. }) { return None; }
    if let Some((owner, signal)) = inner.early_tsf_stop_signal.lock().as_ref() {
        if *owner == session && signal.signal().is_err() { log::warn!("[windows-ime] could not signal the early input stop event"); }
    }
    Some(transition)
}

fn microphone_endpoint_silence_qualified(admission: &EndpointStopAdmission) -> bool {
    let evidence = admission.asr.local_speech_activity_snapshot();
    let captured = admission.asr.endpoint_update_snapshot().audio_duration_ms;
    // Source sample zero is the independently proved end of activation. A
    // wake-only session may have no body speech at all. Require the same full
    // analyzed silence window, current epoch and bounded analysis lag even
    // when the common automatic no-body clock proposes its wall-time escape.
    evidence.state == crate::asr::volcengine::LocalSpeechActivityState::NonSpeech
        && evidence.activity_epoch == admission.ticket.activity_epoch
        && evidence.revision >= admission.ticket.local_vad_revision
        && captured.is_some_and(|captured| captured.saturating_sub(evidence.analyzed_through_ms)
            <= ENDPOINT_STOP_MAX_LOCAL_VAD_LAG_MS)
        && evidence.analyzed_through_ms.saturating_sub(evidence.last_detected_speech_end_ms.unwrap_or(0))
            >= ENDPOINT_STOP_MIN_CONFIRMED_SILENCE_MS
}

async fn finish_microphone_endpoint_stop(
    inner: &Arc<Inner>, reason: &'static str, admission: EndpointStopAdmission,
    stop_dispatched: &Arc<AtomicBool>, stop_completed: &Arc<AtomicBool>,
) {
    let ticket = admission.ticket;
    let Some(transition) = begin_microphone_endpoint_stop(inner, &admission) else {
        if admission.stop_state.lock().cancel_if_current(ticket) {
            stop_dispatched.store(false, Ordering::SeqCst);
            admission.endpoint_clock.lock().reopen_after_failed_stop();
        }
        return;
    };
    admission.stop_state.lock().mark_sent_if_current(ticket);
    stop_completed.store(true, Ordering::SeqCst);
    log::info!("[microphone-endpoint] native stop committed session_id={} proposal_id={} reason={reason}", ticket.session_id, ticket.proposal_id);
    // This is the ordinary native stop: join capture, finalize the same ASR,
    // finish ordinary text delivery, save History, then publish the full copy.
    // It never waits for a device notification and never sends firmware STOP.
    if let Err(error) = finish_end_session_after_stop_transition(inner, transition).await {
        log::warn!("[microphone-endpoint] finalization failed session_id={}: {error}", ticket.session_id);
    }
}

#[cfg(test)]
mod microphone_endpoint_tests {
    use super::*;
    use crate::asr::volcengine::{LocalSpeechActivityState as VadState, LocalSpeechEvidence, VolcengineCredentials};

    fn fixture(manual_vad: bool) -> (super::super::Coordinator, EndpointStopAdmission) {
        let coordinator = super::super::Coordinator::new();
        let inner = &coordinator.inner;
        let session = new_session_id();
        { let mut state = inner.state.lock(); state.session_id=session; state.phase=SessionPhase::Listening; state.cancelled=false; }
        inner.microphone_wake.mark_dictation_owner_for_test(session);
        *inner.recorder.lock() = Some(SessionResource::new(session, Recorder::without_device_for_test()));
        let asr = Arc::new(VolcengineStreamingASR::new(VolcengineCredentials {
            app_id:"offline".into(),access_token:"offline".into(),resource_id:"offline".into(),
        }, Vec::new()));
        asr.note_local_audio_activity_with_speech_end_samples(8000, 128000, Some(4000));
        *asr.local_speech_activity_sink().lock() = LocalSpeechEvidence {
            state:VadState::NonSpeech, revision:12, activity_epoch:2,
            analyzed_through_ms:8000, analyzed_through_samples:128000,
            last_detected_speech_end_ms:Some(4000), trailing_non_speech_ms:Some(4000),
            ..Default::default()
        };
        let clock = Arc::new(Mutex::new(SettledTargetEndpointClock::default()));
        { let mut clock=clock.lock();clock.generation=7;clock.stop_proposed=true; }
        let stop_state = Arc::new(Mutex::new(EndpointStopDispatchState::default()));
        let ticket=stop_state.lock().propose(session,7,12,2,manual_vad,false).unwrap();
        (coordinator,EndpointStopAdmission {asr,endpoint_clock:clock,stop_state,ticket})
    }

    #[test]
    fn automatic_body_lifecycle_preserves_already_trimmed_native_text() {
        let (coordinator, admission)=fixture(true);
        let inner=&coordinator.inner;
        let session=admission.ticket.session_id;
        arm_microphone_wake_body_guard(inner,session);
        let no_body=resolve_target_speaker_endpoint_policy(inner,session,None,Some(0));
        assert!(!no_body.body_started && no_body.initial_body_wait_active);
        assert!(no_body.automatic_no_body_started_at.is_some());
        let text="开始录音这几个字是正文，最后一句也保留。";
        assert_eq!(filter_automatic_wake_text(inner,session,text,true),text);
        let body=resolve_target_speaker_endpoint_policy(inner,session,Some(text),Some(8000));
        assert!(body.body_started && !body.initial_body_wait_active);
        assert!(manual_endpoint_vad_allowed(inner,session,body,&admission.asr.endpoint_update_snapshot()));
        assert_eq!(body.endpoint_timeout_ms,EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS);
        mark_automatic_wake_stop_requested(inner,session);
        assert_eq!(filter_automatic_wake_text(inner,session,text,false),text);
        assert!(!inner.microphone_wake.owns_dictation_session(new_session_id()));
    }

    #[test]
    fn native_stop_rejects_active_stale_missing_or_replaced_capture() {
        for case in 0..11 {
            let (coordinator,admission)=fixture(true);
            let inner=&coordinator.inner;
            match case {
                0=>admission.asr.local_speech_activity_sink().lock().state=VadState::Speech,
                1=>admission.asr.local_speech_activity_sink().lock().state=VadState::PendingSpeech,
                2=>admission.asr.local_speech_activity_sink().lock().state=VadState::Unknown,
                3=>admission.asr.local_speech_activity_sink().lock().activity_epoch+=1,
                4=>admission.asr.local_speech_activity_sink().lock().analyzed_through_ms=7500,
                5=>admission.asr.local_speech_activity_sink().lock().last_detected_speech_end_ms=Some(6000),
                6=>inner.state.lock().session_id=new_session_id(),
                7=>inner.state.lock().phase=SessionPhase::Starting,
                8=>{inner.recorder.lock().take();},
                9=>inner.microphone_wake.mark_dictation_owner_for_test(new_session_id()),
                10=>admission.endpoint_clock.lock().generation+=1,
                _=>unreachable!(),
            }
            assert!(begin_microphone_endpoint_stop(inner,&admission).is_none(),"case {case}");
            assert_ne!(inner.state.lock().phase,SessionPhase::Processing,"case {case}");
        }
    }

    #[test]
    fn native_stop_commits_once_without_device_transport_or_ble_session() {
        let (coordinator,admission)=fixture(true);
        let inner=&coordinator.inner;
        assert!(begin_microphone_endpoint_stop(inner,&admission).is_some());
        assert_eq!(inner.state.lock().phase,SessionPhase::Processing);
        assert!(begin_microphone_endpoint_stop(inner,&admission).is_none());
        // Native recording remains owned until the ordinary finalizer drains it.
        assert!(inner.recorder.lock().as_ref().is_some_and(|r|r.session_id==admission.ticket.session_id));
        assert!(inner.embedded_audio_stats.lock().is_none());
    }

    #[test]
    fn wake_only_stop_needs_full_fresh_analyzed_silence() {
        let (coordinator,admission)=fixture(false);
        { let sink=admission.asr.local_speech_activity_sink();let mut e=sink.lock();e.last_detected_speech_end_ms=None; }
        assert!(begin_microphone_endpoint_stop(&coordinator.inner,&admission).is_some());
        for state in [VadState::Unknown,VadState::PendingSpeech,VadState::Speech] {
            let (_,admission)=fixture(false);
            admission.asr.local_speech_activity_sink().lock().state=state;
            assert!(!microphone_endpoint_silence_qualified(&admission));
        }
    }

    #[test]
    #[ignore = "explicit isolated real VAD and synthetic PCM; no device, cloud or editor"]
    fn synthetic_capture_clock_silence_and_new_speech_revoke_stop() {
        use crate::recorder::AudioConsumer;
        #[derive(Default)] struct Sink(Mutex<Vec<u8>>);
        impl AudioConsumer for Sink {fn consume_pcm_chunk(&self,pcm:&[u8]){self.0.lock().extend_from_slice(pcm);}}
        let (coordinator,mut admission)=fixture(false);
        let asr=&admission.asr;
        asr.prepare_local_speech_quality_session();
        let sink=Arc::new(Sink::default());
        let target:Arc<dyn AudioConsumer>=sink.clone();
        let consumer=microphone_endpoint_consumer(&coordinator.inner,admission.ticket.session_id,asr,target);
        let root=std::path::PathBuf::from(std::env::var_os("LISTENER_WAKE_DIAG_DIR").unwrap());
        let speech=denzic_audio_v1_core::read_wav_pcm16le(&std::fs::read(root.join("wake_huihui.wav")).unwrap()).unwrap();
        let mut input=speech.clone();input.extend(vec![0;5*32000]);
        let mut end=0;
        let wait=|end:u64| {let deadline=Instant::now()+Duration::from_secs(5);while asr.local_speech_activity_snapshot().analyzed_through_samples+512<end {
            assert!(Instant::now()<deadline,"VAD did not advance: {:?}",asr.local_speech_activity_snapshot());std::thread::sleep(Duration::from_millis(2));
        }};
        for chunk in input.chunks(3200) {consumer.consume_pcm_chunk(chunk);end+=chunk.len() as u64/2;wait(end);}
        let quiet=asr.local_speech_activity_snapshot();
        assert_eq!(quiet.state,VadState::NonSpeech);assert!(quiet.last_detected_speech_end_ms.is_some());
        assert_eq!(asr.endpoint_update_snapshot().audio_duration_ms,Some(end/16));
        let ticket=admission.stop_state.lock().ticket.unwrap().1;
        admission.stop_state.lock().cancel_if_current(ticket);
        admission.ticket=admission.stop_state.lock().propose(ticket.session_id,7,quiet.revision,quiet.activity_epoch,false,false).unwrap();
        assert!(microphone_endpoint_silence_qualified(&admission));
        for chunk in speech.chunks(3200) {consumer.consume_pcm_chunk(chunk);end+=chunk.len() as u64/2;wait(end);}
        let active=asr.local_speech_activity_snapshot();
        assert!(active.activity_epoch>quiet.activity_epoch,"new speech must revoke the old STOP epoch: {active:?}");
        assert!(!microphone_endpoint_silence_qualified(&admission));
        input.extend(speech);assert_eq!(*sink.0.lock(),input,"capture is forwarded exactly once without dropping the final chunk");
    }
}
