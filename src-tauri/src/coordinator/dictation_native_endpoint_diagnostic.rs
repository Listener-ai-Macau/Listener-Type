// Explicit one-shot investigation. Not compiled into the product, never opens
// a microphone without a controller OPEN after consent in the shared prompt.
#[test]
#[ignore = "explicit Owner-consented numeric-only microphone endpoint diagnostic"]
fn native_endpoint_v5_readonly_live_diagnostic() {
    use std::io::{BufRead, Write};
    use std::sync::mpsc;
    use crate::asr::volcengine::{LocalSpeechEvidence, VolcengineCredentials};
    let _nonce = std::env::var("LISTENER_NATIVE_ENDPOINT_CONSENT_NONCE").expect("explicit consent controller required");
    let output = PathBuf::from(std::env::var_os("LISTENER_NATIVE_ENDPOINT_OUTPUT").expect("numeric output required"));
    assert!(!output.exists(), "do not overwrite an earlier observation");
    assert!(std::env::var_os("LISTENER_TYPE_ACCEPTANCE_MIC_WAV").is_none());
    let model = crate::speech_vad_windows::ensure_native_endpoint_vad_model().unwrap();
    let root = crate::persistence::speaker_verification_root().unwrap();
    let warm = crate::speech_vad_windows::VadRuntime::load(&root, &model, 0.25).unwrap();
    drop(warm);
    println!("NATIVE_ENDPOINT_COMPONENT_READY"); std::io::stdout().flush().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break; };
            if tx.send(line).is_err() { break; }
        }
    });
    assert_eq!(rx.recv_timeout(Duration::from_secs(1800)).unwrap(), "OPEN");
    struct Meter {
        samples: Mutex<u64>,
        rows: Mutex<Vec<serde_json::Value>>,
        old_activity: Mutex<LocalSpeechActivity>,
    }
    impl crate::recorder::AudioConsumer for Meter {
        fn consume_pcm_chunk(&self, pcm: &[u8]) {
            let mut end = self.samples.lock();
            let start=*end; *end += pcm.len() as u64 / 2;
            self.old_activity.lock().submit(start,*end,pcm);
            let mut sq = 0_f64; let mut mean = 0_f64; let mut peak = 0_i32; let mut nonzero = 0;
            for b in pcm.chunks_exact(2) {
                let v = i16::from_le_bytes([b[0],b[1]]) as i32;
                sq += (v*v) as f64; mean += v as f64; peak = peak.max(v.abs()); nonzero += usize::from(v != 0);
            }
            let n = pcm.len() as f64 / 2.;
            self.rows.lock().push(serde_json::json!({"endSamples":*end,"samples":n,
                "rmsPcm16":(sq/n).sqrt(),"peakPcm16":peak,"meanPcm16":mean/n,"nonzeroSamples":nonzero}));
        }
    }
    let coordinator = super::Coordinator::new();
    let session = new_session_id();
    register_native_microphone_endpoint(&coordinator.inner, session);
    let asr = Arc::new(VolcengineStreamingASR::new(VolcengineCredentials {
        app_id:"offline".into(), access_token:"offline".into(),resource_id:"offline".into(),
    },Vec::new()));
    asr.prepare_local_speech_quality_session();
    let old_sink=Arc::new(Mutex::new(LocalSpeechEvidence::default()));
    let old_activity=LocalSpeechActivity::new(old_sink.clone());
    let meter = Arc::new(Meter {samples:Mutex::new(0),rows:Mutex::new(Vec::new()),old_activity:Mutex::new(old_activity)});
    let consumer = microphone_endpoint_consumer(&coordinator.inner,session,&asr,meter.clone());
    let (recorder, errors, archived) = Recorder::start(None,consumer,Arc::new(|_|{}),None).unwrap();
    assert!(!archived);
    println!("NATIVE_ENDPOINT_MIC_READY"); std::io::stdout().flush().unwrap();
    let started = Instant::now();
    let mut speech_done = None;
    let mut observations = Vec::new();
    let mut previous = LocalSpeechEvidence::default();
    let mut quiet_observation: Option<LocalVadStopObservation> = None;
    let mut first_due_ms = None;
    let mut cancelled = false;
    while started.elapsed() < Duration::from_secs(45) {
        if let Ok(error) = errors.try_recv() { panic!("microphone error: {error}"); }
        while let Ok(command) = rx.try_recv() {
            if command == "DONE" && speech_done.is_none() {
                speech_done = Some(started.elapsed().as_millis() as u64);
                println!("NATIVE_ENDPOINT_QUIET_STARTED"); std::io::stdout().flush().unwrap();
            }
            if command == "CANCEL" { cancelled = true; }
        }
        if cancelled { break; }
        let evidence = asr.local_speech_activity_snapshot();
        let old_evidence=*old_sink.lock();
        let captured_ms = *meter.samples.lock()/16;
        if evidence != previous {
            let now = Instant::now();
            if let Some(o) = quiet_observation.as_mut() { o.observe(evidence,now); }
            else { quiet_observation = Some(LocalVadStopObservation::new(evidence,now)); }
            let due = quiet_observation.as_ref().and_then(LocalVadStopObservation::confirmed_silence)
                .is_some_and(|o|crate::personal_vad::PresenceEndpoint::default().due_on_confirmed_silence(o,captured_ms,now,3000));
            if due && first_due_ms.is_none() { first_due_ms = Some(captured_ms); }
            observations.push(serde_json::json!({"capturedMs":captured_ms,"analyzedSamples":evidence.analyzed_through_samples,
                "revision":evidence.revision,"epoch":evidence.activity_epoch,"state":format!("{:?}",evidence.state),
                "lastSpeechEndMs":evidence.last_detected_speech_end_ms,"pendingStartMs":evidence.pending_speech_start_ms,
                "continuousQuietDue":due,"oldState":format!("{:?}",old_evidence.state),"oldEpoch":old_evidence.activity_epoch,"oldAnalyzedMs":old_evidence.analyzed_through_ms}));
            previous=evidence;
        }
        if speech_done.is_some_and(|at|started.elapsed().as_millis() as u64 >= at+12000) { break; }
        std::thread::sleep(Duration::from_millis(20));
    }
    recorder.stop();
    let completed = speech_done.is_some_and(|at|started.elapsed().as_millis() as u64 >= at+12000) && !cancelled;
    let result=serde_json::json!({"completed":completed,"cancelled":cancelled,
        "speechDoneWallMs":speech_done,"firstQuietDueCapturedMs":first_due_ms,"capturedMs":*meter.samples.lock()/16,
        "audioArchived":false,"networkCalled":false,"editorAccessed":false,"clipboardAccessed":false,
        "source":"production Recorder and MicrophoneEndpointConsumer; candidate v5 and installed legacy on identical production PCM; no saved audio",
        "pcmSignalMetrics":*meter.rows.lock(),"vadObservations":observations});
    std::fs::write(output,serde_json::to_vec(&result).unwrap()).unwrap();
    println!("NATIVE_ENDPOINT_COMPLETE");
}
