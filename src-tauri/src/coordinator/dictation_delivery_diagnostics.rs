// Optional local evidence of submitted text, independent of provider text.
// Included into coordinator::dictation. No editor selection or clipboard read.

fn record_dictation_delivery_diagnostic(
    inner: &Arc<Inner>,
    fact: &DeliveryFact,
    source_key: &str,
    source_coverage: &str,
) {
    log::info!(
        "[delivery] source coverage session_id={} coverage={} source_key_chars={} submitted_chars={:?} target_confirmed={}",
        fact.session_id,
        source_coverage,
        source_key.chars().count(),
        fact.submitted_text.as_ref().map(|text| text.chars().count()),
        fact.target_confirmed,
    );
    if !inner.audio_archive_active.load(Ordering::Relaxed)
        || !record_embedded_audio_for_debug_enabled(inner)
    {
        return;
    }
    let Ok(wav) = crate::persistence::recording_path_for_session(&fact.session_id.to_string()) else {
        return;
    };
    let payload = serde_json::json!({
        "schemaVersion": 1,
        "sessionId": fact.session_id.to_string(),
        "intendedFinalText": fact.intended_text,
        "submittedText": fact.submitted_text,
        "consumedSourceKey": source_key,
        "sourceCoverage": source_coverage,
        "route": fact.route.label(),
        "status": format!("{:?}", fact.status),
        "targetConfirmed": fact.target_confirmed,
    });
    async_runtime::spawn_blocking(move || {
        if !wav.exists() {
            return;
        }
        let path = wav.with_extension("delivery-trace.json");
        let result = serde_json::to_vec(&payload)
            .map_err(|error| error.to_string())
            .and_then(|bytes| std::fs::write(path, bytes).map_err(|error| error.to_string()));
        if let Err(error) = result {
            log::warn!("[delivery] local diagnostic archive failed: {error}");
        }
    });
}
