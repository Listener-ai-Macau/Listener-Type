// Recognizing an already captured prefix and stopping physical recording are
// separate operations. This worker has no UI, owner-clock or final-arbiter
// callback. Its words are eligible only through the current source projection.
const SOURCE_SNAPSHOT_POST_CONTEXT_MS: u64 = 1_000;
const SOURCE_SNAPSHOT_MAX_PCM_MS: u64 = 60_000;
const SOURCE_SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(12);

fn snapshot_clip_bounds(retained_bytes: usize, destination_bytes: u64, owner_end_ms: u64,
    submitted_word: Option<(u64,u64)>, replayed_bytes: usize) -> Option<(usize,u64)> {
    if retained_bytes as u64 != destination_bytes || retained_bytes % 2 != 0 { return None; }
    let through_ms = destination_bytes / 32;
    let origin_ms = match submitted_word {
        Some((start,end)) if start < end && end <= owner_end_ms => start.saturating_sub(SOURCE_SNAPSHOT_POST_CONTEXT_MS),
        Some(_) => return None,
        None => 0,
    };
    let origin = usize::try_from(origin_ms.checked_mul(32)?).ok()?;
    let clip_len = retained_bytes.checked_sub(origin)?;
    if clip_len == 0 || through_ms < owner_end_ms.saturating_add(SOURCE_SNAPSHOT_POST_CONTEXT_MS)
        || through_ms.checked_sub(origin_ms)? > SOURCE_SNAPSHOT_MAX_PCM_MS
        || replayed_bytes.saturating_add(clip_len) > retained_bytes.saturating_mul(2) { return None; }
    Some((origin,through_ms))
}

#[derive(Clone)]
struct SourceSnapshotResult {
    anchor: Option<LiveLedgerAnchor>,
    parent_stream_id: u64,
    parent_lifecycle_generation: u64,
    source_epoch: u64,
    child_decoder_id: u64,
    captured_origin_bytes: u64,
    captured_pcm_bytes: u64,
    captured_through_ms: u64,
    confirmed: crate::transcript_boundary::TimedTranscript,
    completed_at: Instant,
}

struct SourceSnapshotFlight {
    anchor_generation: Option<u64>,
    request_id: u64,
    parent_stream_id: u64,
    source_epoch: u64,
    abort: Option<tokio::task::AbortHandle>,
}

#[derive(Default)]
struct SourceSnapshotState {
    parent_stream_id: Option<u64>,
    closed: bool,
    /// Changes only on capture/transport reopen, not on worker replacement.
    capture_generation: u64,
    lifecycle_generation: u64,
    next_request_id: u64,
    attempted_through_ms: u64,
    attempted_anchor_generation: Option<u64>,
    replayed_pcm_bytes: usize,
    flight: Option<SourceSnapshotFlight>,
    result: Option<SourceSnapshotResult>,
    last_outcome: Option<&'static str>,
}

struct CancelSnapshotDecoder(Arc<VolcengineStreamingASR>);
impl Drop for CancelSnapshotDecoder {
    fn drop(&mut self) { self.0.cancel(); }
}

fn snapshot_timed_prefix(
    timing: &crate::transcript_boundary::TimedTranscript,
    key: &str,
) -> Option<crate::transcript_boundary::TimedTranscript> {
    use crate::transcript_boundary::{content_key, consumed_prefix_end, TimedTranscript};
    if key.is_empty() { return None; }
    let end = consumed_prefix_end(&timing.text, key)?;
    let mut covered = String::new();
    let mut words = Vec::new();
    for word in &timing.words {
        let token = content_key(&word.text);
        if token.is_empty() { continue; }
        if !key.get(covered.len()..)?.starts_with(&token) { return None; }
        covered.push_str(&token);
        words.push(word.clone());
        if covered == key { break; }
    }
    (covered == key).then(|| TimedTranscript { text: timing.text[..end].into(), words })
}

impl VolcengineStreamingASR {
    fn cancel_source_snapshot(&self) {
        #[cfg(target_os = "windows")]
        self.pending_source_identity.reset();
        let mut slot = self.source_snapshot.lock();
        slot.lifecycle_generation = slot.lifecycle_generation.wrapping_add(1);
        if let Some(flight) = slot.flight.take() {
            if let Some(abort) = flight.abort { abort.abort(); }
        }
        slot.result = None;
    }

    fn reset_source_snapshot(&self) {
        self.cancel_source_snapshot();
        #[cfg(target_os = "windows")]
        {
            self.native_source_identity.lock().reset();
            *self.native_word_identity.lock() = Default::default();
        }
        let mut slot = self.source_snapshot.lock();
        // Do not reuse a request id: an old completion may race with reopen.
        let next_request_id = slot.next_request_id;
        let lifecycle_generation = slot.lifecycle_generation;
        let capture_generation = slot.capture_generation.wrapping_add(1);
        *slot = SourceSnapshotState { next_request_id, capture_generation, lifecycle_generation, closed: true, ..Default::default() };
    }

    fn seal_source_snapshot(&self) {
        #[cfg(target_os = "windows")]
        self.pending_source_identity.reset();
        let mut slot = self.source_snapshot.lock();
        slot.closed = true;
        slot.last_outcome = Some("sealed");
        slot.lifecycle_generation = slot.lifecycle_generation.wrapping_add(1);
        if let Some(abort) = slot.flight.take().and_then(|flight| flight.abort) { abort.abort(); }
        slot.result = None;
    }

    fn maybe_start_source_snapshot(&self, candidate: &SelectedLiveOwnerCandidate) {
        self.maybe_start_source_snapshot_from(candidate, None);
    }

    fn maybe_start_source_snapshot_from(&self, candidate: &SelectedLiveOwnerCandidate, anchor: Option<&LiveLedgerAnchor>) {
        use crate::transcript_boundary::content_key;
        if candidate.owner_permission != LiveOwnerPermission::SourceProjection
            || candidate.stability_source.authority != "primary"
            || !candidate.recognition_requires_confirmation
        { return; }
        let body_key = content_key(&candidate.text);
        if candidate.recognition_confirmed_prefix.as_ref()
            .is_some_and(|text| content_key(text) == body_key)
        {
            self.cancel_source_snapshot();
            return;
        }
        // Reuse the existing source-permission maturity. This is not a new
        // endpoint delay and cannot turn unknown audio into silence or Owner.
        if candidate.updated_at.elapsed() < Duration::from_secs(1) { return; }
        let parent_stream_id = candidate.stability_source.decoder;
        let (source_epoch, owner_end_ms, destination_bytes, runtime) = {
            let state = self.state.lock();
            if !state.is_connected || state.finishing || state.owner_isolation_frozen
                || parent_stream_id != self.asr_stream_id()
                || candidate.stability_source.generation != state.owner_delivery_generation
                || state.destination_ledger_incomplete
            { return; }
            let Some(projection) = &state.source_owner_projection_stability else { return; };
            if projection.signature.key != body_key { return; }
            let Some(owner_end_ms) = projection.signature.words.last().and_then(|word| word.2) else { return; };
            let Some(runtime) = state.runtime.clone() else { return; };
            (state.owner_delivery_generation, owner_end_ms, state.next_input_destination_offset, runtime)
        };
        // Do not let the legacy whole-prefix poll cancel a cropped worker.
        // Short captures keep the existing path and its recorded acceptance.
        if (destination_bytes / 32 > SOURCE_SNAPSHOT_MAX_PCM_MS) != anchor.is_some() { return; }
        let (request_id, parent_lifecycle_generation, pcm, captured_origin_bytes, captured_through_ms) = {
            let mut slot = self.source_snapshot.lock();
            if slot.closed || slot.parent_stream_id != Some(parent_stream_id) { return; }
            if anchor.is_some_and(|anchor| anchor.source.capture_stream_id != parent_stream_id
                || anchor.source.capture_generation != slot.capture_generation) { return; }
            let anchor_generation = anchor.map(|anchor| anchor.generation);
            if let Some(flight) = &slot.flight {
                if flight.parent_stream_id == parent_stream_id && flight.source_epoch == source_epoch
                    && flight.anchor_generation == anchor_generation { return; }
                if let Some(abort) = slot.flight.take().and_then(|flight| flight.abort) { abort.abort(); }
            }
            // New provider revisions of already attempted PCM never open
            // another connection, whether the previous attempt passed or failed.
            if owner_end_ms <= slot.attempted_through_ms
                && slot.attempted_anchor_generation == anchor_generation { return; }
            let retained = self.retained_pcm.lock();
            // A transport reopen can reset provider word time while retained
            // PCM still contains the predecessor. Unknown origin is ineligible.
            // A concurrent capture append merely defers this check to next poll.
            let Some((origin,through_ms)) = snapshot_clip_bounds(retained.len(),destination_bytes,
                owner_end_ms,anchor.map(|anchor|anchor.boundary.range_ms()),slot.replayed_pcm_bytes)
                else { return; };
            let captured_origin_bytes = origin as u64;
            let pcm = retained[origin..].to_vec();
            slot.attempted_through_ms = through_ms;
            slot.attempted_anchor_generation = anchor_generation;
            slot.replayed_pcm_bytes = slot.replayed_pcm_bytes.saturating_add(pcm.len());
            slot.next_request_id = slot.next_request_id.wrapping_add(1);
            let request_id = slot.next_request_id;
            slot.flight = Some(SourceSnapshotFlight { anchor_generation, request_id, parent_stream_id, source_epoch, abort: None });
            slot.last_outcome = Some("claimed");
            (request_id, slot.lifecycle_generation, pcm, captured_origin_bytes, through_ms)
        };
        let anchor = anchor.cloned();
        let slot = Arc::clone(&self.source_snapshot);
        let child = Arc::new(Self::new_with_session_options_and_proxy(
            self.credentials.clone(), self.hotwords.clone(), self.session_options, self.proxy_config.clone(),
        ));
        log::info!("[asr] lexical source snapshot claimed parent_stream={parent_stream_id} request={request_id} owner_end_ms={owner_end_ms} captured_origin_bytes={captured_origin_bytes} captured_through_ms={captured_through_ms} anchor_generation={:?}", anchor.as_ref().map(|anchor|anchor.generation));
        let task = runtime.spawn(async move {
            let _cleanup = CancelSnapshotDecoder(Arc::clone(&child));
            let result = tokio::time::timeout(SOURCE_SNAPSHOT_TIMEOUT, async {
                child.open_session().await?;
                child.mark_audio_delivery_ready();
                AudioConsumer::consume_pcm_chunk(&*child, &pcm);
                child.send_last_frame().await?;
                child.await_final_result().await?;
                let state = child.state.lock();
                let confirmed = state.provider_timed_transcript.as_ref().and_then(|timing| {
                    if !state.recognition_boundary.requires_confirmation() { return None; }
                    let prefix = state.recognition_boundary.confirmed_prefix(&timing.text, Some(timing))?;
                    snapshot_timed_prefix(timing, &crate::transcript_boundary::content_key(&prefix))
                });
                Ok::<_, VolcengineASRError>(confirmed.map(|confirmed| SourceSnapshotResult {
                    anchor, parent_stream_id, parent_lifecycle_generation, source_epoch,
                    child_decoder_id: child.asr_stream_id(), captured_origin_bytes,
                    captured_pcm_bytes: pcm.len() as u64,
                    captured_through_ms, confirmed, completed_at: Instant::now(),
                }))
            }).await;
            let mut state = slot.lock();
            if state.flight.as_ref().is_none_or(|flight| flight.request_id != request_id) { return; }
            state.flight = None;
            match result {
                Ok(Ok(Some(result))) => {
                    log::info!("[asr] lexical source snapshot confirmed parent_stream={parent_stream_id} request={request_id} child_stream={} chars={}", result.child_decoder_id, result.confirmed.text.chars().count());
                    state.result = Some(result);
                    state.last_outcome = Some("confirmed");
                }
                Ok(Ok(None)) => { state.last_outcome = Some("no_certificate"); log::info!("[asr] lexical source snapshot has no timed recognition certificate parent_stream={parent_stream_id} request={request_id}"); }
                Ok(Err(error)) => { state.last_outcome = Some("transport_failure"); log::warn!("[asr] lexical source snapshot failed parent_stream={parent_stream_id} request={request_id}: {error}"); }
                Err(_) => { state.last_outcome = Some("timeout"); log::warn!("[asr] lexical source snapshot timed out parent_stream={parent_stream_id} request={request_id}"); }
            }
        });
        let abort = task.abort_handle();
        let mut slot = self.source_snapshot.lock();
        if let Some(flight) = slot.flight.as_mut().filter(|flight| flight.request_id == request_id) {
            flight.abort = Some(abort);
        } else { abort.abort(); }
    }

    fn source_snapshot_delivery_candidate(&self, primary: &SelectedLiveOwnerCandidate) -> Option<SelectedLiveOwnerCandidate> {
        use crate::transcript_boundary::content_key;
        if primary.owner_permission != LiveOwnerPermission::SourceProjection
            || primary.stability_source.authority != "primary"
        { return None; }
        let (snapshot, lifecycle_generation) = {
            let slot = self.source_snapshot.lock();
            if slot.closed || slot.parent_stream_id != Some(self.asr_stream_id()) { return None; }
            (slot.result.clone()?, slot.lifecycle_generation)
        };
        let (source_owner, physical, _) = self.source_owner_evidence_snapshot();
        let state = self.state.lock();
        if !state.is_connected || state.finishing || state.owner_isolation_frozen
            || !enrolled_owner_identity_verified(&state)
            || snapshot.parent_stream_id != self.asr_stream_id()
            || snapshot.source_epoch != state.owner_delivery_generation
            || snapshot.parent_lifecycle_generation != lifecycle_generation
            || snapshot.captured_origin_bytes != 0
            || snapshot.anchor.is_some()
            || snapshot.captured_pcm_bytes == 0 || snapshot.captured_pcm_bytes % 2 != 0
            || snapshot.captured_pcm_bytes / 32 != snapshot.captured_through_ms
            || snapshot.captured_pcm_bytes > state.next_input_destination_offset
            || self.retained_pcm.lock().len() as u64 != state.next_input_destination_offset
            || primary.stability_source.decoder != self.asr_stream_id()
            || primary.stability_source.generation != state.owner_delivery_generation
            || state.destination_ledger_incomplete
        { return None; }
        let (candidate, signature) = primary_source_owner_supported_candidate(&state, &source_owner, &physical)?;
        if content_key(&candidate.text) != content_key(&primary.text) { return None; }
        let (owners, foreign) = source_projection_audio_permissions(&state, &source_owner, &physical);
        // Final and online word spans are different estimates. The child
        // supplies its own confirmed rendering, authorized in its own original
        // PCM coordinates, never a certificate borrowed by the parent's words.
        let child_owned = snapshot_owned_common_prefix(
            &signature, &candidate.text, &snapshot.confirmed,
            (0, snapshot.captured_through_ms), &owners, &foreign,
        )?;
        let primary_confirmed_len = primary.recognition_confirmed_prefix.as_ref().map_or(0, |text| content_key(text).len());
        if content_key(&child_owned.text).len() <= primary_confirmed_len { return None; }
        let text = child_owned.text.clone();
        // Linearize the read against STOP, cancellation and same-object reopen.
        // Child finalization cannot resurrect a result cloned before sealing.
        let slot = self.source_snapshot.lock();
        if slot.closed || slot.lifecycle_generation != lifecycle_generation
            || slot.parent_stream_id != Some(snapshot.parent_stream_id)
        { return None; }
        Some(SelectedLiveOwnerCandidate {
            timing: Some(child_owned),
            recognition_confirmed_prefix: Some(text.clone()), text,
            recognition_requires_confirmation: true,
            owner_permission: LiveOwnerPermission::SourceProjection,
            stability_source: OwnerDeliveryStabilitySource {
                decoder: snapshot.child_decoder_id, generation: snapshot.source_epoch,
                authority: "source_snapshot",
            },
            updated_at: snapshot.completed_at.max(primary.updated_at),
            branch: "source_snapshot", certified_continuation: true,
        })
    }

    pub(crate) fn pause_early_ledger_continuation_after(&self, min_stable: Duration,
        anchor: &LiveLedgerAnchor) -> Option<LiveLedgerContinuation> {
        if self.state.lock().next_input_destination_offset / 32 <= SOURCE_SNAPSHOT_MAX_PCM_MS { return None; }
        let primary = self.selected_live_owner_candidate()?;
        if primary.owner_permission != LiveOwnerPermission::SourceProjection
            || primary.stability_source.authority != "primary"
            || primary.updated_at.elapsed() < min_stable
        { return None; }
        self.maybe_start_source_snapshot_from(&primary, Some(anchor));
        let (snapshot, lifecycle_generation, capture_generation) = {
            let slot = self.source_snapshot.lock();
            if slot.closed || slot.parent_stream_id != Some(self.asr_stream_id())
                || slot.capture_generation != anchor.source.capture_generation
                || self.asr_stream_id() != anchor.source.capture_stream_id { return None; }
            (slot.result.clone()?, slot.lifecycle_generation, slot.capture_generation)
        };
        let bound = snapshot.anchor.as_ref()?;
        if bound.generation != anchor.generation || bound.source_key != anchor.source_key
            || bound.source_text != anchor.source_text || bound.source != anchor.source || bound.boundary != anchor.boundary
            || snapshot.parent_stream_id != anchor.source.capture_stream_id
            || snapshot.parent_lifecycle_generation != lifecycle_generation
            || snapshot.completed_at.elapsed() < min_stable
            || snapshot.captured_origin_bytes % 32 != 0
            || snapshot.captured_pcm_bytes == 0 || snapshot.captured_pcm_bytes % 2 != 0
            || snapshot.captured_origin_bytes.checked_add(snapshot.captured_pcm_bytes)? / 32 != snapshot.captured_through_ms
        { return None; }
        let origin_ms = snapshot.captured_origin_bytes / 32;
        let child = snapshot.confirmed.shifted_from_clip(origin_ms, snapshot.captured_through_ms)?;
        let child_suffix = child.timed_suffix_after_delivery(&anchor.boundary)?;
        let (source_owner, physical, _) = self.source_owner_evidence_snapshot();
        let state = self.state.lock();
        if !state.is_connected || state.finishing || state.owner_isolation_frozen
            || !enrolled_owner_identity_verified(&state) || state.destination_ledger_incomplete
            || snapshot.source_epoch != state.owner_delivery_generation
            || primary.stability_source.generation != state.owner_delivery_generation
            || snapshot.captured_through_ms > state.next_input_destination_offset / 32
            || self.retained_pcm.lock().len() as u64 != state.next_input_destination_offset
        { return None; }
        let (candidate, signature) = primary_source_owner_supported_candidate(&state, &source_owner, &physical)?;
        if crate::transcript_boundary::content_key(&candidate.text) != crate::transcript_boundary::content_key(&primary.text) { return None; }
        let timing = crate::transcript_boundary::TimedTranscript {
            text: candidate.text,
            words: signature.words.into_iter().map(|(text,start_ms,end_ms)|
                crate::transcript_boundary::TimedTranscriptWord { text,start_ms,end_ms }).collect(),
        };
        let parent_suffix = timing.timed_suffix_after_delivery(&anchor.boundary)?;
        let suffix_signature = SourceOwnerProjectionSignature {
            independent_anchor_key: None,
            key: crate::transcript_boundary::content_key(&parent_suffix.text),
            words: parent_suffix.words.iter().map(|word|
                (crate::transcript_boundary::content_key(&word.text),word.start_ms,word.end_ms)).collect(),
        };
        let (owners, foreign) = source_projection_audio_permissions(&state, &source_owner, &physical);
        let confirmed = snapshot_owned_aligned_prefix(&suffix_signature, &parent_suffix.text,
            &child_suffix, (origin_ms,snapshot.captured_through_ms), &owners, &foreign)?;
        let owner_caught_up = crate::transcript_boundary::content_key(&confirmed.text) == suffix_signature.key
            && state.qualified_owner_speech_end_ms.is_some_and(|end| end <= snapshot.captured_through_ms)
            && source_owner.iter().all(|range| range.1 <= snapshot.captured_through_ms);
        let slot = self.source_snapshot.lock();
        if slot.closed || slot.lifecycle_generation != lifecycle_generation
            || slot.capture_generation != capture_generation || self.asr_stream_id() != snapshot.parent_stream_id { return None; }
        Some(LiveLedgerContinuation {
            anchor: anchor.clone(), owner_caught_up, lifecycle_generation,
            suffix: LiveDeliveryTiming { timing: confirmed, source: LiveDeliverySource {
                capture_stream_id: snapshot.parent_stream_id, capture_generation,
                decoder_stream_id: snapshot.child_decoder_id, owner_generation: snapshot.source_epoch,
            } },
        })
    }

    pub(crate) fn ledger_continuation_is_current(&self, continuation: &LiveLedgerContinuation) -> bool {
        use crate::transcript_boundary::{content_key, TimedTranscript, TimedTranscriptWord};
        let (source_owner, physical, _) = self.source_owner_evidence_snapshot();
        let state = self.state.lock();
        let retained_bytes = self.retained_pcm.lock().len() as u64;
        let slot = self.source_snapshot.lock();
        if !(state.is_connected && !state.finishing && !state.owner_isolation_frozen
            && enrolled_owner_identity_verified(&state) && !state.destination_ledger_incomplete
            && retained_bytes == state.next_input_destination_offset
            && state.owner_delivery_generation == continuation.suffix.source.owner_generation
            && !slot.closed && slot.parent_stream_id == Some(self.asr_stream_id())
            && slot.lifecycle_generation == continuation.lifecycle_generation
            && slot.capture_generation == continuation.suffix.source.capture_generation
            && self.asr_stream_id() == continuation.suffix.source.capture_stream_id) { return false; }
        let Some(snapshot) = slot.result.as_ref() else { return false; };
        if snapshot.anchor.as_ref() != Some(&continuation.anchor)
            || snapshot.parent_stream_id != continuation.suffix.source.capture_stream_id
            || snapshot.parent_lifecycle_generation != continuation.lifecycle_generation
            || snapshot.source_epoch != continuation.suffix.source.owner_generation
            || snapshot.child_decoder_id != continuation.suffix.source.decoder_stream_id
            || snapshot.captured_origin_bytes.checked_add(snapshot.captured_pcm_bytes)
                .is_none_or(|end| end / 32 != snapshot.captured_through_ms)
            || snapshot.captured_through_ms > state.next_input_destination_offset / 32
            || (continuation.owner_caught_up && (!state.qualified_owner_speech_end_ms
                .is_some_and(|end| end <= snapshot.captured_through_ms)
                || source_owner.iter().any(|range| range.1 > snapshot.captured_through_ms)))
        { return false; }
        let Some((candidate, signature)) = primary_source_owner_supported_candidate(&state, &source_owner, &physical) else { return false; };
        let current = TimedTranscript { text:candidate.text,
            words:signature.words.into_iter().map(|(text,start_ms,end_ms)|TimedTranscriptWord {text,start_ms,end_ms}).collect() };
        let Some(suffix) = current.timed_suffix_after_delivery(&continuation.anchor.boundary) else { return false; };
        let selected_key = content_key(&continuation.suffix.timing.text);
        let (owners, foreign) = source_projection_audio_permissions(&state, &source_owner, &physical);
        !selected_key.is_empty() && content_key(&suffix.text).starts_with(&selected_key)
            && content_key(&continuation.suffix.timing.owner_supported_prefix("", &owners, &foreign)) == selected_key
    }

    #[cfg(test)]
    fn source_snapshot_diagnostics(&self) -> Value {
        let slot = self.source_snapshot.lock();
        json!({
            "closed":slot.closed, "parent_stream_id":slot.parent_stream_id,
            "lifecycle_generation":slot.lifecycle_generation,
            "capture_generation":slot.capture_generation,
            "attempted_through_ms":slot.attempted_through_ms,
            "replayed_pcm_bytes":slot.replayed_pcm_bytes,
            "in_flight":slot.flight.as_ref().map(|flight|flight.request_id),
            "last_outcome":slot.last_outcome,
            "child":slot.result.as_ref().map(|result| json!({
                "parent_stream_id":result.parent_stream_id, "source_epoch":result.source_epoch,
                "anchor_generation":result.anchor.as_ref().map(|anchor|anchor.generation),
                "parent_lifecycle_generation":result.parent_lifecycle_generation,
                "captured_origin_bytes":result.captured_origin_bytes,"captured_pcm_bytes":result.captured_pcm_bytes,
                "child_decoder_id":result.child_decoder_id, "captured_through_ms":result.captured_through_ms,
                "confirmed_text":result.confirmed.text,
                "words":result.confirmed.words.iter().map(|word|json!({"text":word.text,"start_ms":word.start_ms,"end_ms":word.end_ms})).collect::<Vec<_>>(),
            })),
        })
    }
}
