// Saved failure, application backend only. This entry is compiled explicitly
// and dispatched before single-instance/GUI setup. It owns no device, input
// target, TextInserter, clipboard ticket or physical STOP authority.
pub(crate) fn run_recorded_owner_endpoint_probe() -> i32 {
    let _ = env_logger::Builder::new().filter_level(log::LevelFilter::Warn).try_init();
    match recorded_owner_endpoint_probe() {
        Ok(()) => 0,
        Err(error) => { eprintln!("recorded owner endpoint probe failed: {error}"); 1 }
    }
}

fn recorded_owner_endpoint_probe() -> Result<(), String> {
    use crate::asr::AudioConsumer;
    use serde_json::{json, Value};
    use sha2::{Digest, Sha256};
    let required_path = |name: &str| std::env::var_os(name).map(PathBuf::from)
        .ok_or_else(|| format!("missing explicit {name}"));
    let root = required_path("LISTENER_ENDPOINT_PROBE_ROOT")?;
    let output = required_path("LISTENER_ENDPOINT_PROBE_OUTPUT")?;
    if output.exists() { return Err("probe output already exists".into()); }
    if std::env::var_os("LISTENER_TRACKED_SOURCE_CANDIDATE").is_none() {
        return Err("candidate source path must be explicitly enabled".into());
    }
    // A saved-protocol probe may not silently turn into a paid network replay.
    if std::env::var_os("LISTENER_SOURCE_PREFIX_NETWORK_REPLAY").is_some() {
        return Err("network replay is not permitted in this saved-protocol probe".into());
    }
    let read_json = |path: &Path| -> Result<Value, String> {
        serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
    };
    let protocol_path = root.join("source-protocol.json");
    let protocol = read_json(&protocol_path)?;
    let selected = std::env::var("LISTENER_ENDPOINT_PROBE_CASE").map_err(|e| e.to_string())?;
    let case = protocol["cases"].as_array().and_then(|cases| cases.iter().find(|case| case["id"] == selected))
        .ok_or("saved case missing")?;
    let phrase = protocol["phrase"].as_str().ok_or("saved phrase missing")?;
    let pcm = std::fs::read(case["pcm"].as_str().ok_or("saved PCM path missing")?).map_err(|e| e.to_string())?;
    let pcm_sha = format!("{:x}", Sha256::digest(&pcm));
    if Some(pcm_sha.as_str()) != case["pcmSha256"].as_str() || pcm.len() % 2 != 0 || pcm.len() > 60_000 * 32 {
        return Err("saved PCM binding or bounded probe length invalid".into());
    }
    let native = read_json(Path::new(case["native"].as_str().ok_or("saved native path missing")?))?;
    let packets = native["frames"].as_array().ok_or("saved native frames missing")?.clone();
    if packets.windows(2).any(|pair| pair[0]["capturedMsAtReceipt"].as_f64() > pair[1]["capturedMsAtReceipt"].as_f64()) {
        return Err("saved native receipt order invalid".into());
    }
    let prefix_path = required_path("LISTENER_ENDPOINT_PROBE_PREFIX_RECEIPTS")?;
    let fixture = read_json(&prefix_path)?;
    let receipts = fixture["receipts"].as_array().cloned().unwrap_or_else(|| vec![fixture]);
    for receipt in &receipts {
        let through = receipt["capturedThroughMs"].as_u64().ok_or("prefix boundary missing")?;
        let arrival = receipt["capturedMsAtReceipt"].as_u64().ok_or("prefix receipt missing")?;
        if receipt["fullPcmSha256"] != case["pcmSha256"] || through > arrival || through * 32 > pcm.len() as u64 {
            return Err("prefix PCM or arrival binding invalid".into());
        }
    }
    if receipts.windows(2).any(|pair| pair[0]["capturedMsAtReceipt"].as_u64() > pair[1]["capturedMsAtReceipt"].as_u64()) {
        return Err("saved prefix receipt order invalid".into());
    }
    // A historical successful body may be queried only on the exact retained
    // PCM prefix. This is a read-only consumption comparison, never an editor
    // receipt, a new successful delivery, or an endpoint-clock advance.
    let historical_anchor = std::env::var_os("LISTENER_ENDPOINT_PROBE_HISTORICAL_BODY")
        .map(|path| read_json(Path::new(&path))).transpose()?;
    if let Some(anchor) = historical_anchor.as_ref() {
        let original = std::fs::read(anchor["originalPcm"].as_str().ok_or("anchor original PCM missing")?)
            .map_err(|e|e.to_string())?;
        let original_sha = format!("{:x}",Sha256::digest(&original));
        let through = anchor["capturedMsAtDelivery"].as_u64().ok_or("anchor capture boundary missing")?;
        let bytes = usize::try_from(through).ok().and_then(|v|v.checked_mul(32)).ok_or("anchor prefix overflow")?;
        if anchor["originalPcmSha256"] != original_sha || bytes > original.len() || bytes > pcm.len()
            || original[..bytes] != pcm[..bytes] || anchor["historicalEditorConfirmed"] != true {
            return Err("historical body is not bound to a retained confirmed capture prefix".into());
        }
        let raw = anchor["rawPrefix"].as_str().ok_or("historical raw prefix missing")?;
        let body_start = anchor["bodyStartByte"].as_u64().ok_or("historical body offset missing")? as usize;
        if raw.get(body_start..).is_none() || !raw.starts_with(phrase) {
            return Err("historical body source coordinate invalid".into());
        }
    }
    let profile = crate::speaker_verification::native_word_profile_for_phrase(phrase)?
        .ok_or("protected enrollment missing")?;
    let asr = Arc::new(VolcengineStreamingASR::source_replay_capture_for_test(phrase));
    let epoch = asr.source_replay_epoch_for_test();
    asr.start_personal_vad(None, phrase);
    if !asr.personal_vad_started() { return Err("protected personal detector did not start".into()); }
    let model = root.parent().ok_or("probe parent missing")?
        .join("public-segmentation/sherpa-onnx-pyannote-segmentation-3-0/model.onnx");
    let worker = Arc::new(crate::asr::tracked_source_worker::TrackedSourceWorker::start(
        epoch, phrase.into(), profile, Vec::new(), model,
        asr.source_raw_activity_for_test(), asr.source_evidence_sink_for_test()));
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all()
        .build().map_err(|e| e.to_string())?;
    asr.enable_source_prefix_service_for_test(runtime.handle().clone(), Arc::clone(&worker));
    let (sender, mut receiver) = tokio::sync::mpsc::channel::<Value>(256);
    let receiving_asr = Arc::clone(&asr);
    runtime.spawn(async move {
        while let Some(packet) = receiver.recv().await {
            receiving_asr.apply_saved_native_through_receiver_for_test(&packet).await;
        }
    });
    let start = Instant::now();
    let capture_asr = Arc::clone(&asr);
    let capture_worker = Arc::clone(&worker);
    let handle = runtime.handle().clone();
    let total_ms = pcm.len() as u64 / 32;
    let capture = std::thread::spawn(move || {
        let mut offset = 0;
        let mut native_index = 0;
        let mut prefix_index = 0;
        while offset < pcm.len() {
            let end = (offset + 40 * 32).min(pcm.len());
            let deadline = start + Duration::from_millis(end as u64 / 32);
            if deadline > Instant::now() { std::thread::sleep(deadline.saturating_duration_since(Instant::now())); }
            capture_asr.consume_pcm_chunk(&pcm[offset..end]);
            capture_asr.note_local_audio_arrival(end as u64 / 32);
            // This production entry owns both personal and tracked-source
            // submission. Do not add a second source push from the probe.
            capture_asr.submit_personal_vad(&pcm[offset..end], end as u64 / 2);
            offset = end;
            while native_index < packets.len() && packets[native_index]["capturedMsAtReceipt"].as_f64()
                .is_some_and(|at| at <= end as f64 / 32.0) {
                sender.try_send(packets[native_index]["provider_result"].clone()).expect("saved receiver capacity");
                native_index += 1;
            }
            while let Some(receipt) = receipts.get(prefix_index).filter(|f| f["capturedMsAtReceipt"].as_u64()
                .is_some_and(|at| at <= end as u64 / 32)) {
                handle.block_on(capture_asr.apply_recorded_tracked_prefix_for_test(receipt, Arc::clone(&capture_worker)));
                prefix_index += 1;
            }
        }
        (native_index, prefix_index)
    });
    // Explicit side-effect model, separate from the observational probe. Only
    // confirmed, timed, append-only snapshots can receive a modeled commit.
    // These are not real editor receipts and never authorize a device STOP.
    let model_delivery = std::env::var("LISTENER_ENDPOINT_PROBE_DELIVERY").as_deref() == Ok("modeled-confirmed-append");
    let mut modeled = ModeledProbeDelivery::default();
    let mut clock = SettledTargetEndpointClock { automatic_wake_session: true, ..Default::default() };
    let mut rows = Vec::new();
    let mut historical_consumption = None;
    while start.elapsed() < Duration::from_millis(total_ms + 500) {
        let now = Instant::now();
        let update = asr.endpoint_update_snapshot();
        clock.note_personal_presence_evidence(asr.personal_vad_evidence_snapshot(), now);
        clock.note_local_vad_evidence_at(asr.local_speech_activity_snapshot(), now);
        let selected = asr.pause_early_delivery_evidence_after(Duration::ZERO, "");
        let body_started = selected.as_ref().is_some_and(|s| !crate::transcript_boundary::content_key(&s.text)
            .strip_prefix(&crate::transcript_boundary::content_key(phrase)).unwrap_or(&s.text).is_empty());
        let policy = TargetSpeakerEndpointPolicy { body_started, initial_body_wait_active: !body_started,
            automatic_no_body_started_at: None, endpoint_timeout_ms: 3000, wall_clock_timeout_ms: 3000,
            stop_reason: "recorded_body_probe" };
        // Same first-visible-body seeding as the production watchdog. The
        // session reducer intentionally does not manufacture an initial update.
        if clock.latest_update.is_none() && body_started {
            clock.seed_from_snapshot_if_missing(update.clone(), true, now);
        }
        let modeled_commit = if model_delivery {
            modeled.append_confirmed(selected.as_ref(), phrase, &mut clock, now, start)
        } else { None };
        let pending = if model_delivery {
            modeled.consumed.as_ref().map_or_else(
                || asr.pending_body_delivery_after(""),
                |consumed| asr.pending_body_delivery_from_consumption(consumed),
            )
        } else { asr.pending_body_delivery_after("") };
        clock.refresh_pending_body_delivery(pending, now);
        let proposed = clock.reduce_session_policy(now, policy, &update, asr.owner_analysis_pending_for_endpoint());
        let mut row = clock.personal_endpoint_decision_snapshot(now, 3000);
        row["wallElapsedMs"] = json!(start.elapsed().as_millis());
        row["reducerProposed"] = json!(proposed.is_some());
        row["selectedBody"] = json!(selected.as_ref().map(|s| &s.text));
        row["selectedRecognition"] = json!(selected.as_ref().map(|s| format!("{:?}", s.stability)));
        if model_delivery {
            row["modeledCommit"] = json!(modeled_commit);
            row["modeledDeliveryBlockedBy"] = json!(modeled.blocked_by);
            row["modeledConsumedAudioMs"] = json!(modeled.consumed.as_ref()
                .and_then(|c| c.boundary.as_ref().map(|b| b.range_ms().1)));
        }
        if let Some(anchor) = historical_anchor.as_ref() {
            let mut anchor_state = "not_at_historical_delivery";
            if asr.source_captured_ms_for_test() >= anchor["capturedMsAtDelivery"].as_u64().unwrap() {
                anchor_state = "current_source_prefix_not_aligned";
                if historical_consumption.is_none() {
                    if let Some(snapshot) = selected.as_ref() {
                        let raw = anchor["rawPrefix"].as_str().unwrap();
                        let raw_key = crate::transcript_boundary::content_key(raw);
                        if snapshot.text.starts_with(raw) {
                            if let Some(boundary) = snapshot.timing.as_ref()
                                .filter(|timing| timing.source == snapshot.source)
                                .and_then(|timing|timing.timing.delivery_boundary(&raw_key))
                                .filter(|boundary|boundary.range_ms().1 == anchor["audioEndMs"].as_u64().unwrap()) {
                                let body_start = anchor["bodyStartByte"].as_u64().unwrap() as usize;
                                historical_consumption = Some(crate::asr::volcengine::LiveBodyConsumption {
                                    raw_prefix:raw.into(),body_start,
                                    rendered_key:crate::transcript_boundary::content_key(&raw[body_start..]),
                                    source:snapshot.source,boundary:Some(boundary),
                                });
                            }
                        }
                    }
                }
                if let Some(consumed) = historical_consumption.as_ref() {
                    anchor_state = "exact_historical_prefix_query";
                    let pending = asr.pending_body_delivery_from_consumption(consumed);
                    row["historicalBodyPending"] = json!(pending.as_ref().map(|p|
                        json!({"key":p.key,"recognitionPending":p.recognition_pending})));
                    row["historicalBodyPendingScope"] = asr.tracked_pending_scope_for_test(consumed);
                    row["historicalBodySource"] = json!({"capture":consumed.source.capture_stream_id,
                        "decoder":consumed.source.decoder_stream_id,"boundaryMs":consumed.boundary.as_ref().map(|b|b.range_ms())});
                }
            }
            row["historicalBodyAnchorState"] = json!(anchor_state);
        }
        row["sourceWorker"] = json!(worker.status());
        row["sourcePresenceDecision"] = json!(clock.personal_presence_endpoint.as_ref()
            .map(|p| format!("{:?}", p.decision(asr.source_captured_ms_for_test(), now, 3000))));
        rows.push(row);
        // A diagnostic proposal has no dispatch consumer. Rearm only the
        // proposal latch, never issue STOP or invent a completion receipt.
        if proposed.is_some() { clock.reopen_after_failed_stop(); }
        std::thread::sleep(Duration::from_millis(50));
    }
    let (native_count, prefix_count) = capture.join().map_err(|_| "saved capture worker failed")?;
    // Missing source evidence is itself a relevant result. Do not discard the
    // captured timeline when this contract probe has no evidence to inspect.
    let binding_checks = match asr.source_binding_lifetime_probe() {
        Ok(checks) => checks,
        Err(error) => json!({"notEstablished":error}),
    };
    let result = json!({"kind":"application_backend_saved_failure_probe", "case":selected,
        "construction":case["construction"],"sourceAssessment":case["sourceAssessment"],
        "bindingLifetimeChecks":binding_checks,
        "completedAt":Utc::now().to_rfc3339(), "pcmSha256":pcm_sha, "durationMs":total_ms,
        "nativeReceiptsApplied":native_count, "prefixReceiptsApplied":prefix_count,
        "actualEditor":false,"actualReceipt":false,"actualStop":false,"actualInstalled":false,
        "cloudRequests":0,"profileWrites":0,"runtimeConfiguration":"normal_application_model_settings",
        "historicalConsumptionQueryOnly":historical_anchor.is_some(),
        "deliveryModel":if model_delivery { "modeled-confirmed-append" } else { "none" },
        "modeledCommits":modeled.events,
        "scope":"production source reader, body recognition wait and endpoint reducer; no delivery completion or hardware acceptance",
        "rows":rows});
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&output).map_err(|e| e.to_string())?;
    serde_json::to_writer_pretty(&mut file, &result).map_err(|e| e.to_string())?;
    runtime.shutdown_timeout(Duration::from_millis(500));
    println!("recorded_owner_endpoint_probe_result={}", output.display());
    Ok(())
}

#[derive(Default)]
struct ModeledProbeDelivery {
    consumed: Option<crate::asr::volcengine::LiveBodyConsumption>,
    events: Vec<serde_json::Value>,
    blocked_by: Option<&'static str>,
}

impl ModeledProbeDelivery {
    fn append_confirmed(
        &mut self,
        snapshot: Option<&crate::asr::volcengine::LiveDeliverySnapshot>,
        phrase: &str,
        clock: &mut SettledTargetEndpointClock,
        now: Instant,
        start: Instant,
    ) -> Option<serde_json::Value> {
        use crate::asr::volcengine::{LiveBodyConsumption, LiveDeliveryStability};
        use crate::transcript_boundary::{content_key, consumed_prefix_end};
        self.blocked_by = None;
        let snapshot = snapshot?;
        if snapshot.stability != LiveDeliveryStability::ProviderConfirmed {
            self.blocked_by = Some("recognition_not_confirmed");
            return None;
        }
        let key = content_key(&snapshot.text);
        let Some(timing) = snapshot.timing.as_ref().filter(|t| t.source == snapshot.source) else {
            self.blocked_by = Some("same_source_word_timing_missing");
            return None;
        };
        let Some(boundary) = timing.timing.delivery_boundary(&key) else {
            self.blocked_by = Some("confirmed_prefix_word_boundary_missing");
            return None;
        };
        let (body_start, append_start) = if let Some(prior) = self.consumed.as_ref() {
            if snapshot.source.capture_stream_id != prior.source.capture_stream_id
                || snapshot.source.capture_generation != prior.source.capture_generation {
                self.blocked_by = Some("capture_changed");
                return None;
            }
            if content_key(&prior.raw_prefix) == key { return None; }
            let Some(end) = consumed_prefix_end(&snapshot.text, &content_key(&prior.raw_prefix)) else {
                self.blocked_by = Some("delivered_prefix_revised_unaligned");
                return None;
            };
            if prior.boundary.as_ref().is_none_or(|old| boundary.range_ms().1 <= old.range_ms().1) {
                self.blocked_by = Some("source_occurrence_not_advanced");
                return None;
            }
            (prior.body_start, end)
        } else {
            let end = consumed_prefix_end(&snapshot.text, &content_key(phrase)).unwrap_or(0);
            (end, end)
        };
        let delta = snapshot.text[append_start..].trim();
        if content_key(delta).is_empty() { return None; }
        let sequence = clock.begin_text_submission(now, delta);
        if sequence.is_none() {
            self.blocked_by = Some("body_dispatch_not_admitted");
            return None;
        }
        // Exercise the same dispatch/settle ownership as synchronous product
        // delivery, with an explicit in-memory sink instead of TextInserter.
        clock.settle_text_submission(sequence, true, None);
        self.consumed = Some(LiveBodyConsumption {
            raw_prefix: snapshot.text.clone(), body_start,
            rendered_key: content_key(&snapshot.text[body_start..]),
            source: snapshot.source, boundary: Some(boundary),
        });
        let event = serde_json::json!({"atMs":now.saturating_duration_since(start).as_millis(),
            "text":delta,"audioEndMs":self.consumed.as_ref().unwrap().boundary.as_ref().unwrap().range_ms().1,
            "actualEditor":false,"actualReceipt":false});
        self.events.push(event.clone());
        Some(event)
    }
}

#[cfg(test)]
#[test]
#[ignore = "explicit bound saved failure only; same application backend, no GUI/device/input/STOP"]
fn saved_endpoint_application_backend() {
    assert_eq!(run_recorded_owner_endpoint_probe(), 0);
}
