// Included in coordinator::dictation on Windows. This assessment owns one
// immutable primary hypothesis; the capture actor keeps buffering while it runs.
struct PrimaryKeywordCheck {
    candidate_id: u64,
    range: crate::observability::CandidateRange,
    pcm_sha256: [u8; 32],
    origin_bytes: usize,
    pcm_bytes: usize,
    found: crate::wake_phrase::Match,
    started_at: Instant,
    task: tauri::async_runtime::JoinHandle<PrimaryKeywordCheckResult>,
}

#[derive(Debug)]
struct PrimaryKeywordCheckResult {
    local: Option<(LocalWakeConfirmation, Option<crate::observability::CandidateRange>, usize)>,
    conflict: Option<crate::wake_phrase::AcousticKeywordConflict>,
    error: Option<String>,
}

fn primary_keyword_check_has_same_input(check: &PrimaryKeywordCheck, candidate: &BufferedSpeakerCandidate) -> bool {
    check.candidate_id == candidate.candidate_id
        && slice_candidate_range(candidate.current_candidate_range(), check.origin_bytes, check.pcm_bytes) == Some(check.range)
        && candidate.pcm.get(check.origin_bytes..check.origin_bytes.saturating_add(check.pcm_bytes))
            .is_some_and(|pcm| wake_pcm_digest(pcm) == check.pcm_sha256)
}

fn begin_primary_keyword_check(candidate: &mut BufferedSpeakerCandidate, found: &crate::wake_phrase::Match,
    phrase: &str) -> bool {
    let Some(end) = found.audio_end_seconds.filter(|end| end.is_finite() && *end > 0.0) else { return false; };
    freeze_primary_owner_input(candidate, found);
    let origin = candidate.kws_stream_origin_bytes;
    let Some(pcm) = candidate.pcm.get(origin..) else { return false; };
    let Some(range) = slice_candidate_range(candidate.current_candidate_range(), origin, pcm.len()) else { return false; };
    let expected_end = end - origin as f32 / 32_000.0;
    if expected_end <= 0.0 || expected_end * 32_000.0 > pcm.len() as f32 { return false; }
    if candidate.complete_local_wake_intent.is_some_and(|proof|
        proof.range.start >= range.start && proof.range.end <= range.end
            && proof.range.end >= range.start + (expected_end * 32_000.0).round() as u64) {
        return false;
    }
    let pcm = pcm.to_vec();
    let pcm_sha256 = wake_pcm_digest(&pcm);
    let pcm_bytes = pcm.len();
    let phrase = phrase.to_string();
    let local = candidate.local_confirmation_task.take();
    let local_range = candidate.local_confirmation_task_source_range.take();
    let local_origin = candidate.local_confirmation_task_origin_bytes;
    candidate.local_confirmation_task_started_at = None;
    let task = tauri::async_runtime::spawn(async move {
        let mut observed_local = None;
        if let Some(local) = local {
            if let Ok(Ok(Ok(result))) = tokio::time::timeout(
                Duration::from_millis(TERMINAL_INFLIGHT_CONFIRM_BUDGET_MS), local).await {
                let complete = retain_local_intent_proof(None, result.matched, result.phrase_relation,
                    local_confirmation_source_range(&result, local_range, local_origin))
                    .is_some_and(|proof| proof.range.start >= range.start && proof.range.end <= range.end
                        && proof.range.end >= range.start + (expected_end * 32_000.0).round() as u64);
                observed_local = Some((result, local_range, local_origin));
                if complete { return PrimaryKeywordCheckResult { local: observed_local, conflict: None, error: None }; }
            }
        }
        let comparison = tauri::async_runtime::spawn_blocking(move ||
            crate::asr::local::wake_helper::compare_keyword(&pcm, &phrase, expected_end, Duration::from_secs(4))).await;
        match comparison {
            Ok(Ok(conflict)) => PrimaryKeywordCheckResult { local: observed_local, conflict, error: None },
            Ok(Err(error)) => PrimaryKeywordCheckResult { local: observed_local, conflict: None, error: Some(error) },
            Err(error) => PrimaryKeywordCheckResult { local: observed_local, conflict: None, error: Some(error.to_string()) },
        }
    });
    log::info!("[wake-phrase] primary check queued candidate_id={} source={:?} keyword_end_ms={} actor_wait=false",
        candidate.candidate_id, range, (end * 1000.0).round());
    candidate.primary_keyword_check = Some(PrimaryKeywordCheck {
        candidate_id: candidate.candidate_id, range, pcm_sha256, origin_bytes: origin,
        pcm_bytes, found: found.clone(), started_at: Instant::now(), task,
    });
    true
}

async fn finish_primary_keyword_check(candidate: &mut BufferedSpeakerCandidate, terminal: bool)
    -> Option<crate::wake_phrase::Match> {
    let check = candidate.primary_keyword_check.take()?;
    if !terminal && !check.task.inner().is_finished() {
        candidate.primary_keyword_check = Some(check);
        return None;
    }
    // A replaced/trimmed capture cannot consume this worker's old decision.
    if !primary_keyword_check_has_same_input(&check, candidate) {
        check.task.abort();
        log::info!("[wake-phrase] stale primary check discarded candidate_id={} source={:?}", check.candidate_id, check.range);
        return None;
    }
    let expected_end = check.found.audio_end_seconds.unwrap() - check.origin_bytes as f32 / 32_000.0;
    let outcome = check.task.await;
    let mut conflict = false;
    let mut compute_ms = 0;
    match outcome {
        Ok(result) => {
            if let Some((local, source, origin)) = result.local {
                compute_ms += local.inference_ms;
                candidate.owner_local_phrase_source_range = local_confirmation_source_range(&local, source, origin);
                candidate.retain_complete_local_wake_intent(&local);
            }
            conflict = result.conflict.is_some_and(|proof|
                (proof.keyword_end_seconds * 32_000.0).round() as u64 == (expected_end * 32_000.0).round() as u64);
            if let Some(error) = result.error {
                log::info!("[wake-phrase] primary comparison unavailable candidate_id={} reason={}; keyword retained", check.candidate_id, error);
            }
        }
        Err(error) => log::info!("[wake-phrase] primary comparison task failed candidate_id={} reason={}; keyword retained", check.candidate_id, error),
    }
    log::info!("[wake-phrase] primary check completed candidate_id={} source={:?} conflict={} elapsed_ms={} local_ms={} pcm_retained=true",
        check.candidate_id, check.range, conflict, check.started_at.elapsed().as_millis(), compute_ms);
    if conflict {
        continue_keyword_search_after_conflict(candidate, &check.found);
        None
    } else { Some(check.found) }
}

fn continue_keyword_search_after_conflict(candidate: &mut BufferedSpeakerCandidate, found: &crate::wake_phrase::Match) {
    let end = (found.audio_end_seconds.unwrap() * 32_000.0).round() as usize & !1usize;
    if end <= candidate.kws_stream_origin_bytes || end > candidate.pcm.len() { return; }
    // Only decoder context advances. All original audio stays available to ASR
    // after a later real command; this veto has no text/owner/STOP authority.
    candidate.wake_detector = None;
    let phrase = found.matched_keyword.clone().unwrap();
    candidate.wake_detector_init = Some(tauri::async_runtime::spawn_blocking(move || crate::wake_phrase::StreamingDetector::new(&phrase)));
    candidate.kws_stream_origin_bytes = end;
    candidate.kws_fed_bytes = end;
    candidate.kws_first_hit_at = None;
    candidate.pending_phrase_match = None;
    candidate.primary_owner_input = None;
    candidate.kws_prompted_local_confirm = false;
    candidate.kws_local_absent_count = 0;
    candidate.local_confirmation_attempts = 0;
    candidate.local_confirmation_last_snapshot_bytes = 0;
}

#[cfg(test)]
#[path = "dictation_wake_keyword_check_tests.rs"]
mod primary_keyword_check_tests;
