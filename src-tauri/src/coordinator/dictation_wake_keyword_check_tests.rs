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
        owner_local_phrase_source_range: None, complete_local_wake_intent: None, primary_owner_input: None, primary_keyword_check: None,
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

// The zero prefix is a source-coordinate canary, never an inference fixture.
fn rotated_wake_source_candidate(keyword_pcm: &[u8]) -> (BufferedSpeakerCandidate, crate::wake_phrase::Match) {
    let mut c = candidate(vec![0; 1_782_624]);
    c.kws_stream_origin_bytes = 1_700_416;
    c.kws_fed_bytes = 1_776_832;
    c.pcm[c.kws_stream_origin_bytes..c.kws_fed_bytes].copy_from_slice(keyword_pcm);
    let mut found = primary();
    found.start_seconds = Some(53.138);
    found.end_seconds = 55.138;
    found.audio_end_seconds = Some(55.138);
    (c, found)
}

#[test]
fn wake_identity_source_is_frozen_checked_input_not_whole_prefix() {
    let bytes = vec![7u8; 76_416];
    let (mut c, found) = rotated_wake_source_candidate(&bytes);
    let before = c.pcm.clone();
    freeze_primary_owner_input(&mut c, &found);
    let required = owner_verification_required_wake_range(&c, &found, false).unwrap();
    assert_eq!((required.start, required.end), (1_700_416, 1_776_832));
    let input = wake_verification_input(&c, &found, false).unwrap();
    assert_eq!(input.pcm.as_ref(), bytes);
    assert!(input.same_input(&wake_verification_input(&c,&found,false).unwrap()));
    let mut wrong_candidate=input.clone();wrong_candidate.candidate_id+=1;
    assert!(!wrong_candidate.same_input(&input), "same range/hash does not authorize a different candidate");
    let mut wrong_digest=input.clone();wrong_digest.pcm_sha256[0]^=1;
    assert!(!wrong_digest.same_input(&input));
    assert_eq!(c.pcm, before, "identity input selection must not crop ASR body");
    assert!(!owner_verification_covers_wake(Some(crate::observability::CandidateRange {
        start:0,end:1_764_418}),Some(required)));
    c.pcm.extend_from_slice(&vec![9;32_000]);
    c.kws_fed_bytes=c.pcm.len();
    assert_eq!(wake_verification_input(&c,&found,false).unwrap().pcm.as_ref(),bytes,
        "actor append / terminal feed cannot enlarge an already frozen keyword source");
    c.pcm[1_700_416] ^= 1;
    assert!(wake_verification_input(&c,&found,false).is_err(), "rewritten PCM cannot reuse identity source");
}

#[test]
fn wake_identity_source_rejects_replacement_trim_and_unaligned_ranges() {
    for replacement in [false,true] {
        let (mut c,found)=rotated_wake_source_candidate(&vec![7;76_416]);
        freeze_primary_owner_input(&mut c,&found);
        if replacement {c.candidate_id+=1;} else {c.pcm_base_offset=Some(1_776_832);}
        assert!(wake_verification_input(&c,&found,false).is_err());
    }
    let c=candidate(vec![0;32]);
    for (start,end) in [(1,4),(0,3),(4,4),(0,34)] {
        assert!(WakeVerifierSnapshot::from_candidate_range(&c,crate::observability::CandidateRange{start,end}).is_none());
    }
    let mut found=primary();found.audio_end_seconds=None;
    assert_eq!(wake_verification_input(&c,&found,false).unwrap().pcm.len(),32,
        "observation-clock-only endpoint cannot authorize a keyword crop");
}

fn append_identity_test_source(c: &mut BufferedSpeakerCandidate, start: usize, end: usize, segment: u32) {
    let range = crate::embedded_audio::StreamingPcmRange { start: start as u64, end: end as u64 };
    c.source_runs.push_back(BufferedCandidateSourceRun {
        bytes: end-start, capture_generation: Some(1), segment_id: Some(segment),
        candidate_range: candidate_slice_range(c.pcm_base_offset,start,end-start),
        collector_emitted_range: Some(range), capture_admission_context: None,
        collector_metadata: Some(crate::embedded_audio::StreamingPcmChunkMetadata {
            collector_instance_id: Some(1), segment_ordinal: segment as u64,
            packet_sequence: 0, emission_ordinal: 0, emitted_range: range,
            packet_revision: 0, packet_disposition: crate::embedded_audio::StreamingPcmChunkDisposition::New,
            wire_payload_bytes:end-start, declared_pcm_bytes:end-start, expanded_pcm_bytes:end-start,
            previous_emission_ordinal:None, previous_emitted_range:None,
            revision_conflict:false, metadata_incomplete:false,
        }),
    });
}

fn short_primary_candidate(original: &[u8]) -> (BufferedSpeakerCandidate, crate::wake_phrase::Match) {
    let mut c=candidate(original[..30576].to_vec());
    c.kws_stream_origin_bytes=38;
    c.kws_fed_bytes=30576;
    append_identity_test_source(&mut c,0,30576,2559);
    let mut found=primary();found.audio_end_seconds=Some(0.9);found.end_seconds=0.9;
    freeze_primary_owner_input(&mut c,&found);
    (c,found)
}

#[test]
fn short_keyword_owner_context_is_bounded_same_capture_and_keeps_lexical_receipt() {
    for segment in [2559,2560] {
        let original=vec![7;132000];
        let (mut c,found)=short_primary_candidate(&original);
        let keyword=c.primary_owner_input.as_ref().unwrap().input.clone();
        assert_eq!(wake_verification_input(&c,&found,false).unwrap().pcm.len(),30538);
        c.pcm.extend_from_slice(&original[30576..]);
        append_identity_test_source(&mut c,30576,original.len(),segment);
        let input=wake_verification_input(&c,&found,false).unwrap();
        assert_eq!(input.pcm.len(),if segment==2559 {35200} else {30538});
        if segment==2559 {
            let frozen=c.primary_owner_input.take();
            assert!(input.same_input(&wake_verification_input(&c,&found,false).unwrap()),
                "terminal-first detection uses the same bounded context");
            c.primary_owner_input=frozen;
        }
        assert!(keyword.same_input(&c.primary_owner_input.as_ref().unwrap().input));
        c.pcm.extend_from_slice(&vec![4;32000]);
        assert!(input.same_input(&wake_verification_input(&c,&found,false).unwrap()),
            "future body must not become an unbounded owner retry input");
        c.pcm[38]^=1;
        assert!(wake_verification_input(&c,&found,false).is_err());
    }
}

#[tokio::test]
#[ignore = "private saved 09:12 wake through actual protected verifier; no cloud/device/editor/profile writes"]
async fn short_keyword_saved_failure_reaches_same_capture_verifier() {
    use sha2::Digest;
    let original=denzic_audio_v1_core::read_wav_pcm16le(&std::fs::read(
        std::env::var("LISTENER_SHORT_WAKE_INPUT").unwrap()).unwrap()).unwrap();
    let phrase="开始录音";
    assert!(crate::speaker_verification::has_bound_template_without_mutation(phrase));
    let (mut c,found)=short_primary_candidate(&original);
    let before=wake_verification_input(&c,&found,false).unwrap();
    let short_pcm=before.pcm.clone();
    let short_result=tauri::async_runtime::spawn_blocking(move ||
        crate::speaker_verification::verify(&short_pcm,phrase)).await.unwrap();
    assert!(short_result.unwrap_err().contains("shorter than"));
    c.pcm.extend_from_slice(&original[30576..]);
    append_identity_test_source(&mut c,30576,original.len(),2559);
    refresh_owner_verification_prefetch_for_wake(&mut c,phrase,1808532559,&found);
    let prefetched=c.owner_verification_task.take().unwrap();
    let input=prefetched.input;
    let (result,compute_ms)=prefetched.task.await.unwrap();
    let result=result.unwrap();
    let (_,decision,_)=arbitrate_candidate_wake(&mut c,
        denzic_voice_activation_v1_core::PhraseSignal::KeywordModel,Some(&found),phrase,
        &Ok(result.clone()),false,false,input.pcm.len()/32);
    std::fs::write(std::env::var("LISTENER_SHORT_WAKE_OUTPUT").unwrap(),
        serde_json::to_vec_pretty(&serde_json::json!({"sourceSession":1808532559,
            "originalPcmSha256":format!("{:x}",sha2::Sha256::digest(&original)),
            "oldInputMs":before.pcm.len()/32,"newInputMs":input.pcm.len()/32,
            "score":result.score,"matched":result.matched,"decision":format!("{:?}",decision.decision),
            "computeMs":compute_ms,"scope":"Saved real PCM and production prefetch/arbitration; source metadata reconstructed from original log, not fresh device acceptance"})).unwrap()).unwrap();
    assert!(result.enrolled_owner_matched(),"saved owner phrase must pass unchanged enrolled policy");
    assert!(decision.owner_access.enrolled_owner_verified());
}

#[tokio::test]
#[ignore = "retained exact private keyword PCM and already-bound bank; no cloud, editor, device or profile mutation"]
async fn wake_identity_source_saved_input_reaches_actual_prefetched_verifier() {
    let _=env_logger::builder().is_test(true).filter_level(log::LevelFilter::Info).try_init();
    let fixture:serde_json::Value=serde_json::from_slice(&std::fs::read(
        std::env::var("LISTENER_CURRENT_WAKE_INPUT").unwrap()).unwrap()).unwrap();
    let bytes=std::fs::read(fixture["verification_pcm_path"].as_str().unwrap()).unwrap();
    use sha2::Digest;
    let hash=format!("{:x}",sha2::Sha256::digest(&bytes));
    assert_eq!(hash,fixture["verification_pcm_sha256"].as_str().unwrap());
    let phrase=fixture["wake_phrase"].as_str().unwrap();
    assert!(crate::speaker_verification::has_bound_template_without_mutation(phrase),
        "already-bound bank required; do not invoke legacy phrase-binding writes");
    let (mut c,found)=rotated_wake_source_candidate(&bytes);
    refresh_owner_verification_prefetch_for_wake(&mut c,phrase,0,&found);
    let prefetch=c.owner_verification_task.take().expect("actual production prefetch");
    assert_eq!(prefetch.input.pcm.as_ref(),bytes);
    assert_eq!(prefetch.input.pcm_sha256,wake_pcm_digest(&bytes));
    let ((verification,compute_ms),input)=(prefetch.task.await.unwrap(),prefetch.input);
    let result=verification.unwrap();
    assert!(matches!(result.policy,crate::speaker_verification::VerificationPolicy::Enrolled));
    // Canary prefix has no capture receipt and cannot become a direct Owner seed.
    let direct=input.observe_direct_result(&Ok(result.clone()),&c).is_some();
    assert!(!direct);
    std::fs::write(std::env::var("LISTENER_CURRENT_WAKE_OUTPUT").unwrap(),
        serde_json::to_vec_pretty(&serde_json::json!({"schema":"listener.bound-keyword-verifier.v1",
        "source_session":"8ae1f493-2fba-42c3-9580-7dddfc1785d8","pcm_sha256":hash,
        "actual_verifier_pcm_ms":bytes.len()/32,"source_start_byte":1_700_416,"source_end_byte":1_776_832,
        "score":result.score,"matched":result.matched,"compute_ms":compute_ms,"direct_source_retained":direct,
        "scope":"Actual protected verifier through production prefetch, exact byte-proven archived keyword input only. Synthetic prefix used solely to reproduce metadata selection; never inferred, not an end-to-end wake/ASR/STOP or latency comparison."})).unwrap()).unwrap();
}

#[tokio::test]
async fn owner_prefetch_lane_never_replaces_running_work_and_weak_hits_do_not_rebind() {
    let mut c = candidate(vec![5; 35264]);
    let input = WakeVerifierSnapshot::from_candidate(&c);
    let range = input.range;
    let (send, receive) = tokio::sync::oneshot::channel::<()>();
    c.owner_verification_task = Some(PrefetchedOwnerVerification {
        source_range: range, input: input.clone(),
        task: tauri::async_runtime::spawn(async move {
            receive.await.unwrap();
            (Ok(crate::speaker_verification::VerificationResult {
                matched: true, owner_matched: true, score: 0.58,
                policy: crate::speaker_verification::VerificationPolicy::Enrolled,
            }), 1)
        }),
    });
    let mut weak = primary(); weak.matched_keyword = Some("开始录".into());
    for _ in 0..44 {
        c.pcm.extend_from_slice(&vec![3; 1632]);
        // Exact producer seam, irrespective of whether a phrase is enrolled.
        let next = WakeVerifierSnapshot::from_candidate(&c);
        start_owner_verification_with_input(&mut c, "开始录音", 3308, next);
        refresh_owner_verification_prefetch_for_wake(&mut c, "开始录音", 3308, &weak);
        assert!(c.owner_verification_task.as_ref().unwrap().input.same_input(&input));
    }
    send.send(()).unwrap();
    let (returned, result, _) = join_candidate_owner_verification(&mut c, input.clone(), "开始录音").await;
    assert!(returned.same_input(&input));
    assert!(result.unwrap().enrolled_owner_matched());
    assert!(c.owner_verification_task.is_none());
}

#[tokio::test]
#[ignore = "saved 15:13 wake and protected existing profile; no cloud/device/editor writes"]
async fn saved_slow_wake_runs_bounded_owner_lane_on_exact_local_phrase_source() {
    // The test profile otherwise creates a new model directory for every PID.
    // Resource downloading/extraction would corrupt the wake-latency comparison.
    let cached_root = std::path::PathBuf::from(std::env::var_os("LISTENER_SPEAKER_MODEL_ROOT")
        .expect("saved wake replay requires the explicit existing speaker resource directory"));
    for name in ["3dspeaker_speech_campplus_sv_zh-cn_16k-common.onnx", "onnxruntime.dll",
        "onnxruntime_providers_shared.dll", "sherpa-onnx-c-api.dll"] {
        assert!(cached_root.join(name).is_file(), "saved replay cannot prepare missing resources: {name}");
    }
    let _ = env_logger::builder().is_test(true).filter_level(log::LevelFilter::Info).try_init();
    let fixture:serde_json::Value = serde_json::from_slice(&std::fs::read(
        std::env::var("LISTENER_SLOW_WAKE_FIXTURE").unwrap()).unwrap()).unwrap();
    let pcm = denzic_audio_v1_core::read_wav_pcm16le(&std::fs::read(
        fixture["wav"].as_str().unwrap()).unwrap()).unwrap();
    let phrase = "开始录音";
    assert!(crate::speaker_verification::has_bound_template_without_mutation(phrase));
    let mut c = candidate(pcm[..35264].to_vec());
    append_identity_test_source(&mut c, 0, 35264, 3308);
    maybe_prefetch_owner_verification(&mut c, phrase, 1808533308);
    let initial = c.owner_verification_task.as_ref().unwrap().input.clone();
    let mut weak = primary(); weak.matched_keyword = Some("开始录".into());
    weak.audio_end_seconds = Some(1.720);
    let mut last = c.pcm.len();
    for end in fixture["replacementEnds"].as_array().unwrap().iter().map(|x|x.as_u64().unwrap() as usize) {
        if end > last { c.pcm.extend_from_slice(&pcm[last..end]); append_identity_test_source(&mut c,last,end,3308); last=end; }
        refresh_owner_verification_prefetch_for_wake(&mut c, phrase, 1808533308, &weak);
        assert!(c.owner_verification_task.as_ref().unwrap().input.same_input(&initial));
    }
    // Local ASR actually confirmed the 3416ms frozen source, not later body.
    c.owner_local_phrase_source_range = Some(crate::observability::CandidateRange { start:0, end:3416*32 });
    let mut local = primary(); local.audio_end_seconds = None; local.matched_keyword = None;
    let expected = wake_verification_input(&c, &local, false).unwrap();
    assert_eq!(expected.pcm.as_ref(), &pcm[..3416*32]);
    let began = Instant::now();
    let (verified, result, compute) = join_candidate_owner_verification(&mut c, expected, phrase).await;
    let result = result.unwrap();
    assert!(result.enrolled_owner_matched(), "the same real owner source must retain the unchanged enrolled policy");
    std::fs::write(std::env::var("LISTENER_SLOW_WAKE_REPORT").unwrap(), serde_json::to_vec_pretty(
        &serde_json::json!({"sourceSession":1808533308,"oldTasks":45,"oldReplacements":44,
            "weakRebindings":0,"sourceMs":verified.pcm.len()/32,"sourceDigest":verified.pcm_sha256,
            "ownerMatched":result.matched,"score":result.score,"computeMs":compute,
            "joinWallMs":began.elapsed().as_millis(),"scope":"Saved PCM, actual protected verifier and production lane; no new现场 or editor acceptance"})).unwrap()).unwrap();
}

#[test]
#[ignore = "same saved failure in cold and warm process state; no new scene/cloud/device/editor"]
fn saved_slow_wake_latency_separates_cold_initialization_from_warm_lane() {
    let output=std::path::PathBuf::from(std::env::var("LISTENER_SLOW_WAKE_REPORT").unwrap());
    saved_slow_wake_runs_bounded_owner_lane_on_exact_local_phrase_source();
    let cold:serde_json::Value=serde_json::from_slice(&std::fs::read(&output).unwrap()).unwrap();
    std::fs::write(output.with_file_name("saved-slow-wake-cold.json"),serde_json::to_vec_pretty(&cold).unwrap()).unwrap();
    saved_slow_wake_runs_bounded_owner_lane_on_exact_local_phrase_source();
    let warm:serde_json::Value=serde_json::from_slice(&std::fs::read(&output).unwrap()).unwrap();
    std::fs::write(output.with_file_name("saved-slow-wake-cold-warm.json"),serde_json::to_vec_pretty(
        &serde_json::json!({"cold":cold,"warm":warm,"oldLiveJoinMs":6593,
            "scope":"Same saved failure, production lane, exact input and existing protected profile; startup separated, no live device claim"})).unwrap()).unwrap();
    assert_eq!(cold["score"],warm["score"]);
}
