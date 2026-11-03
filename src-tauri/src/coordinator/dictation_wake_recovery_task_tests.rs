use super::*;

#[test]
fn wake_recovery_keeps_ingress_available_until_phrase_and_owner_are_both_finished() {
    tauri::async_runtime::block_on(async {
        let (entered, started) = tokio::sync::oneshot::channel();
        let (release, finish) = tokio::sync::oneshot::channel();
        let task = WakeRecoveryTask::spawn(|| Ok(()), move |_| async move {
            let _ = entered.send(());
            finish.await.map_err(|_| "cancelled".to_string())?;
            Ok(42)
        });
        tokio::time::timeout(Duration::from_secs(2), started).await.unwrap().unwrap();
        // Separation is ready, but verification is deliberately stalled. The
        // same completion condition used by the live actor must stay false.
        for _ in 0..32 {
            assert!(!task.is_finished());
            tokio::task::yield_now().await;
        }
        release.send(()).unwrap();
        assert_eq!(task.result().await.unwrap(), 42);
    });
}

#[test]
fn wake_recovery_retiring_candidate_cancels_pending_evaluation() {
    tauri::async_runtime::block_on(async {
        let (entered, started) = tokio::sync::oneshot::channel();
        let (mut release, finish) = tokio::sync::oneshot::channel::<()>();
        let task = WakeRecoveryTask::spawn(|| Ok(()), move |_| async move {
            let _ = entered.send(());
            let _ = finish.await;
            Ok(())
        });
        tokio::time::timeout(Duration::from_secs(2), started).await.unwrap().unwrap();
        drop(task);
        tokio::time::timeout(Duration::from_secs(2), release.closed()).await.unwrap();
    });
}

#[test]
fn wake_recovery_terminal_budget_still_applies_to_extraction_not_verification() {
    tauri::async_runtime::block_on(async {
        let (entered, started) = tokio::sync::oneshot::channel();
        let (release, finish) = tokio::sync::oneshot::channel();
        let task = WakeRecoveryTask::spawn(|| Ok(()), move |_| async move {
            let _ = entered.send(());
            finish.await.map_err(|_| "cancelled".to_string())?;
            Ok(7)
        });
        started.await.unwrap();
        let result = task.terminal_result(1);
        tokio::pin!(result);
        tokio::select! {
            _ = &mut result => panic!("completed extraction must keep its verification budget"),
            _ = tokio::time::sleep(Duration::from_millis(20)) => {}
        }
        release.send(()).unwrap();
        assert_eq!(result.await.unwrap(), 7);
    });
}

#[test]
fn wake_recovery_expired_extraction_cannot_start_a_stale_helper_request() {
    use std::sync::atomic::{AtomicBool, Ordering};
    tauri::async_runtime::block_on(async {
        let (release, hold) = std::sync::mpsc::channel();
        let entered = Arc::new(AtomicBool::new(false));
        let evaluated = Arc::clone(&entered);
        let task = WakeRecoveryTask::spawn(move || {
            hold.recv_timeout(Duration::from_secs(2)).map_err(|e| e.to_string())?;
            Ok(())
        }, move |_| async move {
            evaluated.store(true, Ordering::SeqCst);
            Ok(())
        });
        assert_eq!(task.terminal_result(10).await.unwrap_err(), "wake_extraction_budget_exceeded");
        // The native extraction may finish after retirement. It must not
        // consume the live helper for the now-abandoned audio snapshot.
        let _ = release.send(());
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(!entered.load(Ordering::SeqCst));
    });
}

#[test]
fn wake_recovery_background_evaluation_still_requires_phrase_and_owner() {
    for phrase_matched in [false, true] {
        for owner in [false, true] {
            let evaluated = EvaluatedTargetWakeExtraction {
                phrase_matched,
                evidence: ExtractedOwnerWakeEvidence {
                    wake_match: crate::wake_phrase::Match {
                        start_seconds: None, end_seconds: 1.8, matched_keyword: None,
                        origin: crate::wake_phrase::KeywordEvidenceOrigin::PrimaryDetector,
                        audio_end_seconds: None,
                    },
                    local_confirmation_ms: 20, extraction_ms: 100,
                    owner_score: if owner { 0.7 } else { 0.1 },
                    owner_verified_by_extraction: owner, residual_ratio: 0.1,
                },
            };
            assert_eq!(qualify_target_wake_extraction(evaluated, false).is_some(), phrase_matched && owner);
        }
    }
}

#[test]
fn wake_recovery_live_actor_polls_full_recovery_without_inline_model_evaluation() {
    let source = include_str!("dictation_embedded_stream.rs");
    let live = source.split("async fn try_release_automatic_candidate(").nth(1).unwrap();
    assert!(live.contains("task.is_finished()"));
    assert!(live.contains("task.result().await"));
    assert!(!live.contains("evaluate_target_wake_extraction("));
}
