use super::*;
fn candidate(pcm: Vec<u8>) -> BufferedSpeakerCandidate {
    BufferedSpeakerCandidate {
        candidate_id: 65,
        kind: BufferedSpeakerCandidateKind::Verification,
        pcm,
        pcm_base_offset: Some(0),
        candidate_cursor: PcmDiagnosticCursor::default(),
        source_runs: VecDeque::new(),
        source_admission_ledger: Arc::new(std::sync::Mutex::new(SourceAdmissionDependencyLedger::default())),
        fact_ledger: crate::observability::CandidateFactLedger::default(),
        next_operation_id: Some(1), source_admission_operation_id: Some(1),
        wake_detector: None, wake_detector_init: None, pending_phrase_match: None,
        owner_ambiguous_confirmations: 0, owner_best_ambiguous_score: 0.0,
        owner_verification_task: None, owner_verification_attempted: false,
        owner_local_phrase_source_range: None, complete_local_wake_intent: None, primary_keyword_check: None,
        #[cfg(feature = "target-speaker-extraction")]
        target_wake_extraction_task: None,
        #[cfg(feature = "target-speaker-extraction")]
        target_wake_extraction_attempted: false,
        local_confirmation_task: None, local_confirmation_task_started_at: None,
        local_confirmation_window_origin_bytes: 0, local_confirmation_task_origin_bytes: 0,
        local_confirmation_task_source_range: None, local_confirmation_task_has_keyword_model_hit: false,
        local_confirmation_prefix_retry: LocalConfirmationPrefixRetryState::default(),
        local_near_retry: LocalNearRetryState::default(),
        local_confirmation_attempts: 2, local_confirmation_last_snapshot_bytes: 56000,
        kws_prompted_local_confirm: true, kws_local_absent_count: 1,
        local_absent_count: 1, local_kws_fusion_evidence: false, kws_phrase_detected: false,
        local_owner_overlap_near_confirmations: 0, owner_near_phrase_confirmations: 0,
        local_owner_masked_phrase_evidence: false,
        wake_interference_baseline: WakeInterferenceBaseline::default(), local_absent_coverage: None,
        kws_first_hit_at: Some(Instant::now()), kws_first_hit_pcm_ms: Some(1750),
        early_capsule_session_id: None, early_capsule_request_ms: None,
        kws_fed_bytes: 0, kws_stream_origin_bytes: 0, kws_total_ms: 0,
        started_at: Instant::now(),
    }
}
fn primary() -> crate::wake_phrase::Match {
    crate::wake_phrase::Match { start_seconds: Some(0.0), end_seconds: 1.76,
        audio_end_seconds: Some(1.76), matched_keyword: Some("开始录音".into()),
        origin: crate::wake_phrase::KeywordEvidenceOrigin::PrimaryDetector }
}
fn pending_check(c: &BufferedSpeakerCandidate, task: tauri::async_runtime::JoinHandle<PrimaryKeywordCheckResult>) -> PrimaryKeywordCheck {
    PrimaryKeywordCheck { candidate_id: c.candidate_id,
        range: c.current_candidate_range().unwrap(), pcm_sha256: wake_pcm_digest(&c.pcm),
        origin_bytes: 0, pcm_bytes: c.pcm.len(), found: primary(), started_at: Instant::now(), task }
}
#[tokio::test]
async fn pending_assessment_releases_actor_and_does_not_consume_new_audio() {
    let mut c = candidate(vec![0; 67200]);
    let (send, receive) = tokio::sync::oneshot::channel::<()>();
    let task = tauri::async_runtime::spawn(async move {
        receive.await.unwrap();
        PrimaryKeywordCheckResult { local: None, conflict: None, error: None }
    });
    c.primary_keyword_check = Some(pending_check(&c, task));
    assert!(finish_primary_keyword_check(&mut c, false).await.is_none());
    assert!(c.primary_keyword_check.is_some());
    c.pcm.extend_from_slice(&[2,3,4,5]);
    assert!(primary_keyword_check_has_same_input(c.primary_keyword_check.as_ref().unwrap(), &c));
    send.send(()).unwrap();
    assert!(finish_primary_keyword_check(&mut c, true).await.is_some());
    assert_eq!(c.pcm.len(), 67204);
    assert_eq!(&c.pcm[67200..], &[2,3,4,5]);
}
#[tokio::test]
async fn modified_replaced_and_trimmed_input_cannot_apply_an_old_veto() {
    for change in 0..3 {
        let mut c = candidate(vec![0; 67200]);
        let task = tauri::async_runtime::spawn(async {
            PrimaryKeywordCheckResult { local: None, error: None,
                conflict: Some(crate::wake_phrase::AcousticKeywordConflict {
                    keyword_end_seconds: 1.76, alternative_end_seconds: 1.64, shared_anchor_seconds: vec![0.84] }) }
        });
        c.primary_keyword_check = Some(pending_check(&c, task));
        match change { 0 => c.candidate_id += 1, 1 => c.pcm[0] = 1, _ => c.pcm_base_offset = Some(2) }
        assert!(finish_primary_keyword_check(&mut c, true).await.is_none());
        assert_eq!(c.kws_stream_origin_bytes,0);
    }
}
#[tokio::test]
async fn worker_unavailable_retains_keyword_without_negative_authority() {
    let mut c = candidate(vec![0; 67200]);
    let task = tauri::async_runtime::spawn(async {
        PrimaryKeywordCheckResult { local: None, conflict: None, error: Some("helper unavailable".into()) }
    });
    c.primary_keyword_check = Some(pending_check(&c, task));
    assert!(finish_primary_keyword_check(&mut c, true).await.is_some());
    assert_eq!(c.kws_stream_origin_bytes,0);
}
#[test]
fn completed_same_input_local_intent_needs_no_second_job() {
    let mut c = candidate(vec![0; 67200]);
    c.complete_local_wake_intent = Some(CompleteLocalWakeIntent { range: crate::observability::CandidateRange { start:0, end:67200 } });
    assert!(!begin_primary_keyword_check(&mut c,&primary(),"开始录音"));
    assert!(c.primary_keyword_check.is_none());
}
#[tokio::test]
#[ignore = "actual retained false command through production helper and candidate admission; no cloud/editor/device"]
async fn saved_false_command_crosses_helper_and_both_candidate_completion_paths() {
    let _ = env_logger::builder().is_test(true).filter_level(log::LevelFilter::Info).try_init();
    let path = std::env::var("LISTENER_WAKE_DIAG_CLIP").unwrap();
    let wav = std::fs::read(path).unwrap();
    let original = denzic_audio_v1_core::read_wav_pcm16le(&wav).unwrap();
    crate::asr::local::wake_helper::preload().unwrap();
    for terminal in [false,true] {
        let mut c = candidate(original.clone());
        assert!(begin_primary_keyword_check(&mut c,&primary(),"开始录音"));
        if !terminal {
            while !c.primary_keyword_check.as_ref().unwrap().task.inner().is_finished() {
                assert!(finish_primary_keyword_check(&mut c,false).await.is_none());
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
        assert!(finish_primary_keyword_check(&mut c,terminal).await.is_none());
        assert_eq!(c.pcm, original);
        assert_eq!(c.kws_stream_origin_bytes,56320,"only confirmed wrong hypothesis advances the decoder");
        assert_eq!(c.local_confirmation_window_origin_bytes,0);
        assert!(c.pending_phrase_match.is_none());
        assert!(c.early_capsule_session_id.is_none());
        let mut detector = c.wake_detector_init.take().unwrap().await.unwrap().unwrap();
        let remainder = &c.pcm[c.kws_fed_bytes..];
        let followup = detector.accept_pcm(remainder).unwrap();
        println!("saved_false_command terminal={terminal} bytes={} next_search={} followup={followup:?} capsule=false stop=false",c.pcm.len(),c.kws_fed_bytes);
    }
}
