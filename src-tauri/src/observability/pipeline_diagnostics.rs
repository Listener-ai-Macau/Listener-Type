//! Small live records, with complete bounded ledgers exported only on request.
//! Serialization and disk I/O for detailed evidence never run for normal PCM
//! or segment lifecycle notifications. The ledgers themselves are unchanged.
use super::*;
use std::io::Write;
use std::path::Path;

const EXPORT_LIMIT: usize = 8 * 1024 * 1024;
static LAST_ENDED: OnceLock<Mutex<Option<Arc<EmbeddedAudioPipelineObservation>>>> = OnceLock::new();
static EXPORT_LOCK: Mutex<()> = Mutex::new(());

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticExport<'a> {
    schema_version: u32,
    scope: &'static str,
    process_id: u32,
    captures: &'a [PipelineSnapshot],
}

pub(super) fn remember_ended(observation: &Arc<EmbeddedAudioPipelineObservation>) {
    *LAST_ENDED.get_or_init(|| Mutex::new(None)).lock() = Some(Arc::clone(observation));
}

impl EmbeddedAudioPipelineObservation {
    pub(super) fn production_summaries(
        &self,
        reason: &'static str,
        force: bool,
    ) -> Vec<PipelineSummarySnapshot> {
        let now = Instant::now();
        let mut state = self.state.lock();
        if !force
            && state
                .last_snapshot_at
                .is_some_and(|last| now.duration_since(last) < Duration::from_secs(1))
        {
            return Vec::new();
        }
        state.last_snapshot_at = Some(now);
        let current = state.embedded_session_id;
        let newly_settled = state
            .segment_counters
            .iter()
            .filter_map(|(id, counters)| {
                (segment_counters_settled(counters)
                    && !state.segment_final_counter_logged.contains(id))
                .then_some(*id)
            })
            .collect::<Vec<_>>();
        // Preserve final per-segment counters before bounded ledger eviction.
        // Late ASR completions can settle a predecessor after a new segment starts.
        let mut targets = vec![(reason, current)];
        targets.extend(
            newly_settled
                .iter()
                .copied()
                .filter(|id| Some(*id) != current)
                .map(|id| ("segment_settled", Some(id))),
        );
        state
            .segment_final_snapshot_emitted
            .extend(newly_settled.iter().copied());
        state.segment_final_counter_logged.extend(newly_settled);
        targets
            .into_iter()
            .map(|(reason, segment_id)| {
                state.snapshot_seq = state.snapshot_seq.saturating_add(1);
                PipelineSummarySnapshot {
                    event: "embedded_audio_pipeline_snapshot",
                    detail: "summary",
                    reason,
                    full_detail_source: "export_audio_pipeline_diagnostics",
                    capture_generation: self.capture_generation,
                    coordinator_session_id: state.coordinator_session_id.clone(),
                    embedded_session_id: current,
                    segment_snapshot_id: segment_id,
                    snapshot_seq: state.snapshot_seq,
                    monotonic_ms: now.duration_since(state.started_at).as_millis() as u64,
                    counters: state.counters,
                    segment_counters: segment_id
                        .and_then(|id| {
                            state
                                .segment_counters
                                .get(&id)
                                .copied()
                                .map(|counter| (id, counter))
                        })
                        .into_iter()
                        .collect(),
                    segment_ledger_incomplete: state.segment_ledger_incomplete,
                    interval_ledger_incomplete: state.interval_ledger_incomplete,
                    asr_destination_facts_incomplete: state.asr_destination_facts_incomplete,
                    pcm_stage_facts_incomplete: state.pcm_stage_ledger.capacity_drops().4,
                    collector_pcm_facts_incomplete: state.collector_pcm_facts_incomplete,
                    candidate_facts_incomplete: state.candidate_ledger.capacity_drops().4,
                }
            })
            .collect()
    }
}

struct BoundedJson(Vec<u8>);
impl Write for BoundedJson {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > EXPORT_LIMIT.saturating_sub(self.0.len()) {
            return Err(std::io::Error::other(
                "audio pipeline diagnostic exceeds 8 MiB",
            ));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn encode_bounded(payload: &impl Serialize) -> Result<Vec<u8>, String> {
    let mut encoded = BoundedJson(Vec::new());
    serde_json::to_writer(&mut encoded, payload).map_err(|error| error.to_string())?;
    Ok(encoded.0)
}

#[cfg(test)]
fn write_export(path: &Path, payload: &impl Serialize) -> Result<usize, String> {
    write_export_bytes(path, &encode_bounded(payload)?)
}

fn write_export_bytes(path: &Path, encoded: &[u8]) -> Result<usize, String> {
    // Encoding has completed before this point. Failed encoding or file I/O
    // never advertises a partial JSON file as the new complete evidence.
    let parent = path
        .parent()
        .ok_or("audio pipeline diagnostic path has no parent")?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temporary = parent.join(format!(".audio-pipeline-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| error.to_string())?;
        file.write_all(encoded).map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        drop(file);
        std::fs::rename(&temporary, path).map_err(|error| error.to_string())?;
        Ok(encoded.len())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

fn build_export_bytes() -> Result<(Vec<u8>, Vec<u64>), String> {
    let mut observations = PIPELINE_OBSERVATIONS
        .get()
        .map(|registry| registry.lock().values().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    if observations.len() > 4 {
        return Err("too many active audio pipeline captures to export".into());
    }
    if let Some(previous) = LAST_ENDED.get().and_then(|last| last.lock().clone()) {
        if !observations
            .iter()
            .any(|current| Arc::ptr_eq(current, &previous))
        {
            observations.push(previous);
        }
    }
    observations.sort_by_key(|observation| observation.capture_generation);
    let snapshots = observations
        .iter()
        .map(|observation| {
            full_snapshot(
                observation.capture_generation,
                "explicit_diagnostic_export",
                Instant::now(),
                &observation.state.lock(),
            )
        })
        .collect::<Vec<_>>();
    let generations = observations
        .iter()
        .map(|observation| observation.capture_generation)
        .collect::<Vec<_>>();
    // Serialize from the owned snapshot once; do not duplicate a large ledger
    // into serde_json::Value before applying the output/memory bound.
    let payload = DiagnosticExport {
        schema_version: 1,
        scope: "Current bounded in-memory ledgers and most recent ended capture; not a complete lifetime trace. Inspect all incomplete/drop flags.",
        process_id: std::process::id(),
        captures: &snapshots,
    };
    Ok((encode_bounded(&payload)?, generations))
}

/// Used by the existing user-initiated support ZIP, without creating a sidecar
/// file or requiring a developer connection to the application.
pub(crate) fn audio_pipeline_diagnostic_bytes() -> Result<Vec<u8>, String> {
    build_export_bytes().map(|(bytes, _)| bytes)
}

/// Called by the main-window diagnostic command on a blocking worker, never
/// by capture callbacks. Only the latest export is retained (8 MiB maximum).
pub(crate) fn export_audio_pipeline_diagnostics() -> Result<serde_json::Value, String> {
    let _export = EXPORT_LOCK.lock();
    let (encoded, generations) = build_export_bytes()?;
    let path = crate::log_dir_path().join("audio-pipeline-diagnostics/latest.json");
    let bytes = write_export_bytes(&path, &encoded)?;
    Ok(
        serde_json::json!({"path": path, "bytes": bytes, "limitBytes": EXPORT_LIMIT,
        "captureGenerations": generations, "completeJson": true}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn populated_observation() -> EmbeddedAudioPipelineObservation {
        let observation = EmbeddedAudioPipelineObservation::new(131);
        let share = AsrSourceShareFact {
            segment_id: Some(17),
            bytes: 320,
            destination_range: Some(PcmRange { start: 0, end: 320 }),
            source_interval: None,
            collector_metadata: None,
            collector_emitted_range: None,
        };
        let mut state = observation.state.lock();
        state.embedded_session_id = Some(17);
        state
            .segment_counters
            .insert(17, PipelineCounters::default());
        state.asr_destination_facts_incomplete = true;
        for sequence in 0..ASR_DESTINATION_FACT_CAPACITY {
            state.asr_destination_facts.push_back(AsrDestinationFact {
                asr_stream_id: 3,
                sequence: Some(sequence as i32),
                destination_range: Some(PcmRange {
                    start: sequence as u64 * 2560,
                    end: sequence as u64 * 2560 + 2560,
                }),
                outcome: AsrDestinationOutcome::SocketSendCompleted,
                source_shares: vec![share.clone(); 8],
            });
        }
        drop(state);
        observation
    }

    #[test]
    fn routine_forced_snapshots_stay_parseable_and_keep_large_detail_available() {
        let observation = populated_observation();
        let detail = full_snapshot(
            131,
            "explicit_diagnostic_export",
            Instant::now(),
            &observation.state.lock(),
        );
        let detail_json = serde_json::to_value(&detail).unwrap();
        // The real failure: an otherwise legal ledger exceeds the logger's
        // per-record limit. Clipping that JSON at 64 KiB destroys its structure.
        let original = serde_json::to_vec(&detail).unwrap();
        assert!(original.len() > 65_536);
        assert!(serde_json::from_slice::<serde_json::Value>(&original[..65_536]).is_err());
        let mut largest_summary = 0;
        for reason in [
            "capture_started",
            "capture_stopped",
            "coordinator_started",
            "coordinator_stopped",
            "coordinator_terminal_drained",
            "capture_error",
        ] {
            let records = observation.production_summaries(reason, true);
            assert_eq!(records.len(), 1);
            let encoded = serde_json::to_vec(&records[0]).unwrap();
            largest_summary = largest_summary.max(encoded.len());
            assert!(
                encoded.len() < 8_192,
                "routine record grew with detail: {}",
                encoded.len()
            );
            let parsed: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
            assert_eq!(parsed["reason"], reason);
            assert_eq!(parsed["detail"], "summary");
            assert_eq!(parsed["asr_destination_facts_incomplete"], true);
            assert_eq!(
                parsed["full_detail_source"],
                "export_audio_pipeline_diagnostics"
            );
        }
        println!(
            "pipeline_snapshot_bytes before_full={} after_largest_summary={} retained_asr_facts={}",
            original.len(),
            largest_summary,
            ASR_DESTINATION_FACT_CAPACITY
        );
        let after = full_snapshot(
            131,
            "explicit_diagnostic_export",
            Instant::now(),
            &observation.state.lock(),
        );
        assert_eq!(
            serde_json::to_value(after).unwrap()["asr_destination_facts"],
            detail_json["asr_destination_facts"]
        );
    }

    #[test]
    fn late_settled_predecessor_has_its_own_final_counter_record() {
        let observation = EmbeddedAudioPipelineObservation::new(132);
        {
            let mut state = observation.state.lock();
            state.embedded_session_id = Some(18);
            state
                .segment_counters
                .insert(18, PipelineCounters::default());
            let mut predecessor = PipelineCounters::default();
            predecessor.capture_stopped_count = 1;
            predecessor.coordinator_terminal_drained_count = 1;
            state.segment_counters.insert(17, predecessor);
            // Delivery may have claimed its final record before the drain.
            state.segment_final_snapshot_emitted.insert(17);
        }
        let records = observation.production_summaries("coordinator_terminal_drained", true);
        assert_eq!(records.len(), 2);
        let final_record = records
            .iter()
            .find(|record| record.reason == "segment_settled")
            .unwrap();
        assert_eq!(final_record.segment_snapshot_id, Some(17));
        assert_eq!(
            final_record.segment_counters[&17].coordinator_terminal_drained_count,
            1
        );
        assert_eq!(records[0].embedded_session_id, Some(18));
        assert_eq!(
            observation
                .production_summaries("capture_started", true)
                .len(),
            1
        );
    }

    #[test]
    fn full_export_is_complete_above_log_limit_and_replaces_one_file() {
        let observation = populated_observation();
        let folder =
            std::env::temp_dir().join(format!("listener-pipeline-export-{}", uuid::Uuid::new_v4()));
        let path = folder.join("latest.json");
        let snapshot = full_snapshot(
            131,
            "explicit_diagnostic_export",
            Instant::now(),
            &observation.state.lock(),
        );
        let snapshots = [snapshot];
        let export = DiagnosticExport {
            schema_version: 1,
            scope: "Synthetic bounded test ledger",
            process_id: 0,
            captures: &snapshots,
        };
        let bytes = write_export(&path, &export).unwrap();
        assert!(bytes > 65_536 && bytes < EXPORT_LIMIT);
        let parsed: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            parsed["captures"][0]["asr_destination_facts"]
                .as_array()
                .unwrap()
                .len(),
            ASR_DESTINATION_FACT_CAPACITY
        );
        assert_eq!(parsed["schemaVersion"], 1);
        write_export(&path, &export).unwrap();
        assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 1);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(folder).unwrap();
    }

    #[test]
    fn oversize_export_retains_previous_complete_file() {
        let folder =
            std::env::temp_dir().join(format!("listener-pipeline-limit-{}", uuid::Uuid::new_v4()));
        let path = folder.join("latest.json");
        write_export(&path, &serde_json::json!({"previous": true})).unwrap();
        let original = std::fs::read(&path).unwrap();
        assert!(write_export(&path, &"x".repeat(EXPORT_LIMIT + 1)).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 1);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(folder).unwrap();
    }
}

pub(super) fn full_snapshot(
    capture_generation: u64,
    reason: &'static str,
    now: Instant,
    state: &PipelineObservationState,
) -> PipelineSnapshot {
    PipelineSnapshot {
        event: "embedded_audio_pipeline_snapshot",
        reason,
        capture_generation,
        coordinator_session_id: state.coordinator_session_id.clone(),
        embedded_session_id: state.embedded_session_id,
        snapshot_seq: state.snapshot_seq,
        monotonic_ms: now.duration_since(state.started_at).as_millis() as u64,
        counters: state.counters,
        segment_counters: state.segment_counters.clone(),
        segment_ledger_overflow_count: state.segment_ledger_overflow_count,
        segment_ledger_overflow_bytes: state.segment_ledger_overflow_bytes,
        segment_ledger_incomplete: state.segment_ledger_incomplete,
        interval_ledger_overflow_count: state.interval_ledger_overflow_count,
        interval_ledger_overflow_bytes: state.interval_ledger_overflow_bytes,
        interval_ledger_incomplete: state.interval_ledger_incomplete,
        asr_destination_facts: state.asr_destination_facts.iter().cloned().collect(),
        asr_destination_fact_update_drop_count: state.asr_destination_fact_update_drop_count,
        asr_destination_fact_update_drop_bytes: state.asr_destination_fact_update_drop_bytes,
        asr_destination_share_drop_count: state.asr_destination_share_drop_count,
        asr_destination_share_drop_bytes: state.asr_destination_share_drop_bytes,
        asr_destination_fact_missing_metadata_count: state
            .asr_destination_fact_missing_metadata_count,
        asr_destination_fact_missing_metadata_bytes: state
            .asr_destination_fact_missing_metadata_bytes,
        asr_destination_fact_conflict_count: state.asr_destination_fact_conflict_count,
        asr_destination_fact_conflict_bytes: state.asr_destination_fact_conflict_bytes,
        asr_destination_facts_incomplete: state.asr_destination_facts_incomplete,
        pcm_stage_facts: state.pcm_stage_ledger.facts(),
        pcm_stage_fact_update_drop_count: state.pcm_stage_ledger.capacity_drops().0,
        pcm_stage_fact_update_drop_bytes: state.pcm_stage_ledger.capacity_drops().1,
        pcm_stage_share_drop_count: state.pcm_stage_ledger.capacity_drops().2,
        pcm_stage_share_drop_bytes: state.pcm_stage_ledger.capacity_drops().3,
        pcm_stage_facts_incomplete: state.pcm_stage_ledger.capacity_drops().4,
        collector_pcm_facts: state.collector_pcm_facts.iter().copied().collect(),
        collector_pcm_fact_drop_count: state.collector_pcm_fact_drop_count,
        collector_pcm_fact_drop_bytes: state.collector_pcm_fact_drop_bytes,
        collector_pcm_facts_incomplete: state.collector_pcm_facts_incomplete,
        candidate_facts: state.candidate_ledger.facts(),
        candidate_fact_drop_count: state.candidate_ledger.capacity_drops().0,
        candidate_fact_drop_bytes: state.candidate_ledger.capacity_drops().1,
        candidate_source_run_drop_count: state.candidate_ledger.capacity_drops().2,
        candidate_source_run_drop_bytes: state.candidate_ledger.capacity_drops().3,
        candidate_facts_incomplete: state.candidate_ledger.capacity_drops().4,
    }
}
