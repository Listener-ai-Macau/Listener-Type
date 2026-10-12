//! Fixed Windows CPU adapter; the endpoint reducer is platform independent.
use super::{fuse_presence, latest_target_source_end, PresenceAudioBuffer, PresenceObservation, PresenceState, PresenceVerifier, OWNER_ACTIVITY_MIN_PROBABILITY};
use ort::{session::Session, value::TensorRef};
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    time::Instant,
};

const DETECTOR: &str = "nomo-pvad-2.4s-fp32.onnx";
const ENCODER: &str = "eres2netv2-nomo-enrollment-fp32.onnx";
const DETECTOR_SHA: &str = "ff39bd6dcff75264c70910713547708959da539c22a2f551fdb94cf9af5d86dc";
pub(crate) const ENCODER_SHA: &str = "9ee27ce50a44ff2cb13bc4f22eac67114fa11ea4768a43a1e6fb2798172bce58";
const CHUNK_BYTES: usize = 160 * 32;
const WINDOW_BYTES: usize = 2_400 * 32;

#[derive(Clone, Debug)]
pub(crate) struct PersonalVadReference(Vec<f32>);

impl PersonalVadReference {
    pub(crate) fn from_bound_values(model_sha: &str, frontend: &str, mut values: Vec<f32>) -> Result<Self, String> {
        if model_sha != ENCODER_SHA || frontend != crate::speaker_identity_eres::FRONTEND_ID
            || values.len() != 192 || values.iter().any(|value| !value.is_finite())
        { return Err("personal activity reference model, frontend or values are incompatible".into()); }
        let norm = values.iter().map(|value| (*value as f64).powi(2)).sum::<f64>().sqrt();
        if !norm.is_finite() || norm <= f64::EPSILON { return Err("personal activity reference is empty".into()); }
        for value in &mut values { *value = (*value as f64 / norm) as f32; }
        Ok(Self(values))
    }
    pub(crate) fn values(&self) -> &[f32] { &self.0 }
}

pub(crate) fn enrollment_reference(pcm: &[u8]) -> Result<PersonalVadReference, String> {
    let values = runtime()?.lock().reference(pcm)?;
    PersonalVadReference::from_bound_values(ENCODER_SHA, crate::speaker_identity_eres::FRONTEND_ID, values)
}

struct Runtime {
    encoder: Option<Session>,
    detector: Session,
}

fn model_path(name: &str) -> Result<std::path::PathBuf, String> {
    #[cfg(test)]
    if let Some(root) = std::env::var_os("LISTENER_PVAD_FIXTURES") {
        return Ok(std::path::PathBuf::from(root).join(name));
    }
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let installed = executable
        .parent()
        .ok_or("personal VAD executable has no parent")?
        .join("resources/models")
        .join(name);
    if installed.is_file() {
        return Ok(installed);
    }
    Ok(std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("resources/models")
        .join(name))
}

fn load_session(path: &Path, expected_hash: &str) -> Result<Session, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("read personal VAD model: {e}"))?;
    if format!("{:x}", Sha256::digest(&bytes)) != expected_hash {
        return Err("personal VAD model hash is incompatible".into());
    }
    Session::builder()
        .map_err(|e| e.to_string())?
        .with_intra_threads(2)
        .map_err(|e| e.to_string())?
        .with_inter_threads(1)
        .map_err(|e| e.to_string())?
        .commit_from_memory(&bytes)
        .map_err(|e| format!("load personal VAD model: {e}"))
}

impl Runtime {
    fn load() -> Result<Self, String> {
        let root = crate::persistence::speaker_verification_root().map_err(|e| e.to_string())?;
        let dll = root.join("onnxruntime.dll");
        let initializer =
            ort::init_from(&dll).map_err(|e| format!("load personal VAD runtime: {e}"))?;
        let _ = initializer.with_name("listener-owner-activity").commit();
        Ok(Self {
            // A protected enrollment already supplies the bound reference.
            // Streaming detection does not use this separate encoder model.
            encoder: None,
            detector: load_session(&model_path(DETECTOR)?, DETECTOR_SHA)?,
        })
    }

    fn reference(&mut self, pcm: &[u8]) -> Result<Vec<f32>, String> {
        let (frames, values) = crate::speaker_identity_eres::fbank(pcm)?;
        if self.encoder.is_none() {
            self.encoder = Some(load_session(&model_path(ENCODER)?, ENCODER_SHA)?);
        }
        let input = TensorRef::from_array_view(([1usize, frames, 80usize], values.as_slice()))
            .map_err(|e| e.to_string())?;
        let output = self
            .encoder
            .as_mut()
            .ok_or("personal activity reference encoder unavailable")?
            .run(ort::inputs!["fbank" => input])
            .map_err(|e| e.to_string())?;
        let (shape, embedding) = output["embedding"]
            .try_extract_tensor::<f32>()
            .map_err(|e| e.to_string())?;
        if shape.as_ref() != [1i64, 192] || embedding.iter().any(|v| !v.is_finite()) {
            return Err("invalid personal VAD reference".into());
        }
        let norm = embedding
            .iter()
            .map(|v| (*v as f64).powi(2))
            .sum::<f64>()
            .sqrt();
        if !norm.is_finite() || norm <= f64::EPSILON {
            return Err("empty personal VAD reference".into());
        }
        Ok(embedding
            .iter()
            .map(|v| (*v as f64 / norm) as f32)
            .collect())
    }

    fn probabilities(&mut self, reference: &[f32], pcm: &[u8]) -> Result<Vec<f32>, String> {
        if pcm.len() < CHUNK_BYTES || pcm.len() > WINDOW_BYTES || pcm.len() % CHUNK_BYTES != 0 {
            return Err("personal VAD input is outside the fixed streaming export".into());
        }
        let (frames, values) = if pcm.iter().all(|v| *v == 0) {
            let frames = pcm.len() / 320 - 2;
            (frames, vec![0.0; frames * 80])
        } else {
            crate::speaker_identity_eres::fbank(pcm)?
        };
        let audio = TensorRef::from_array_view(([1usize, frames, 80usize], values.as_slice()))
            .map_err(|e| e.to_string())?;
        let speaker = TensorRef::from_array_view(([1usize, 192usize], reference))
            .map_err(|e| e.to_string())?;
        let output = self
            .detector
            .run(ort::inputs!["speaker_embedding" => speaker, "fbank" => audio])
            .map_err(|e| format!("run personal VAD: {e}"))?;
        let (shape, probabilities) = output["target_probability"]
            .try_extract_tensor::<f32>()
            .map_err(|e| e.to_string())?;
        if shape.as_ref() != [1i64, (pcm.len() / CHUNK_BYTES) as i64]
            || probabilities.iter().any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
        {
            return Err("personal VAD returned invalid chunk coverage or probabilities".into());
        }
        Ok(probabilities.to_vec())
    }

    #[cfg(test)]
    fn probability(&mut self, reference: &[f32], pcm: &[u8]) -> Result<f32, String> {
        self.probabilities(reference, pcm)?.last().copied().ok_or("personal VAD returned no probability".into())
    }
}

fn runtime() -> Result<Arc<Mutex<Runtime>>, String> {
    static SHARED: once_cell::sync::Lazy<Mutex<Option<Arc<Mutex<Runtime>>>>> =
        once_cell::sync::Lazy::new(|| Mutex::new(None));
    let mut slot = SHARED.lock();
    if let Some(loaded) = slot.as_ref() {
        return Ok(Arc::clone(loaded));
    }
    let loaded = Arc::new(Mutex::new(Runtime::load()?));
    *slot = Some(Arc::clone(&loaded));
    Ok(loaded)
}

pub(crate) struct PersonalVadStream {
    sender: Option<mpsc::SyncSender<(Vec<u8>, u64)>>,
    latest: Arc<Mutex<Option<PresenceObservation>>>,
    unavailable: Arc<AtomicBool>,
    timeline: Arc<Mutex<crate::asr::volcengine_speaker_ledger::LocalSpeakerTimeline>>,
    raw_activity: Arc<Mutex<crate::asr::volcengine_speaker_ledger::LocalSpeakerTimeline>>,
    endpoint_source: Arc<Mutex<super::EndpointSourceTimeline>>,
    profile_binding: Option<(String, [u8; 32])>,
}

impl PersonalVadStream {
    /// Only an independently verified wake/reference may be passed here.
    /// No dictation audio ever updates or persists this reference.
    pub(crate) fn start(reference_pcm: Vec<u8>, enrolled_reference: Option<PersonalVadReference>, mut verifier: Option<PresenceVerifier>) -> Self {
        let (sender, receiver) = mpsc::sync_channel::<(Vec<u8>, u64)>(200);
        let latest = Arc::new(Mutex::new(None));
        let unavailable = Arc::new(AtomicBool::new(false));
        let timeline = Arc::new(Mutex::new(crate::asr::volcengine_speaker_ledger::LocalSpeakerTimeline::default()));
        let source_timeline = Arc::clone(&timeline);
        let raw_activity = Arc::new(Mutex::new(crate::asr::volcengine_speaker_ledger::LocalSpeakerTimeline::default()));
        let raw_output = Arc::clone(&raw_activity);
        let endpoint_source = Arc::new(Mutex::new(super::EndpointSourceTimeline::default()));
        let endpoint_output = Arc::clone(&endpoint_source);
        let output = Arc::clone(&latest);
        let failed = Arc::clone(&unavailable);
        tauri::async_runtime::spawn_blocking(move || {
            #[cfg(test)]
            eprintln!("[saved-presence] worker entered");
            let work = || -> Result<(), String> {
                let loaded = runtime()?;
                #[cfg(test)]
                eprintln!("[saved-presence] runtime loaded");
                let source = if enrolled_reference.is_some() { "three_sample_enrollment" } else { "legacy_verified_wake" };
                let reference = match enrolled_reference {
                    Some(reference) => reference.0,
                    None => loaded.lock().reference(&reference_pcm)?,
                };
                log::info!("[owner-activity] reference ready source={source} verified_wake_pcm_ms={} output=all_chunks", reference_pcm.len()/32);
                let mut audio = PresenceAudioBuffer::default();
                let mut positive_end = None;
                let mut last_diagnostic_ms = 0;
                loop {
                    if failed.load(Ordering::Acquire) {
                        return Ok(());
                    }
                    if !audio.has_complete_chunk() {
                        let Ok((pcm, source_end_samples)) = receiver.recv() else { break; };
                        audio.push(&pcm, source_end_samples)?;
                    }
                    // Inference takes longer than one capture message on a
                    // busy CPU. Drain available input before the next run;
                    // every new chunk gets its own existing model output.
                    // There is no batching timer and no dropped input audio.
                    while let Ok((pcm, source_end_samples)) = receiver.try_recv() {
                        audio.push(&pcm, source_end_samples)?;
                    }
                    if positive_end.is_none() {
                        positive_end = Some(audio.origin_ms());
                    }
                    let Some(window) = audio.next_window() else { continue; };
                    #[cfg(test)]
                    eprintln!("[saved-presence] analyzing first_end={} new_chunks={}", window.first_new_end_samples, window.new_chunks);
                    let compute_started = Instant::now();
                    // Both independent scorers consume this exact immutable
                    // source window. Serial inference would make every other-
                    // speaker observation stale before endpoint admission.
                    // Join before advancing the buffer: no result can migrate
                    // to a later window, and at most one identity run is live.
                    let (scores, identity) = std::thread::scope(|scope| {
                        let identity = verifier.as_mut().and_then(|check| {
                            std::thread::Builder::new()
                                .name("listener-endpoint-identity".into())
                                .spawn_scoped(scope, || check(&window.pcm).ok())
                                .ok()
                        });
                        let scores = loaded.lock().probabilities(&reference, &window.pcm);
                        let identity = identity.and_then(|task| task.join().ok().flatten());
                        (scores, identity)
                    });
                    let scores = scores?;
                    #[cfg(test)]
                    eprintln!("[saved-presence] window complete");
                    // Keep detector positives separate from a rolling-window
                    // identity rescue. The rescue protects recording lifetime;
                    // it does not locate each speaker's words within a mixture.
                    super::publish_scored_window(&mut raw_output.lock(), &scores,
                        window.first_new_end_samples, window.new_chunks, None)?;
                    let new_scores = &scores[scores.len() - window.new_chunks..];
                    let mut source_evidence = source_timeline.lock();
                    super::publish_scored_window(&mut source_evidence,&scores,
                        window.first_new_end_samples,window.new_chunks,identity.map(|identity| identity.projection))?;
                    // The native-word projection's coarse mismatch is not an
                    // absence certificate. Keep complete endpoint history at
                    // the same source coordinates with its own qualified tier.
                    let mut endpoint_evidence = endpoint_output.lock();
                    endpoint_evidence.note_scored_window(&scores,
                        window.first_new_end_samples,window.new_chunks,identity)?;
                    // Preserve positive context at its own source position.
                    // Labeling every new chunk Unknown because of an older
                    // positive would move that old speech to the current time.
                    // Its original frontier already vetoes STOP for three
                    // seconds. Per-chunk fusion keeps owner precedence while
                    // retaining independently confirmed Other for newer audio.
                    positive_end = positive_end.max(latest_target_source_end(
                        &scores, window.first_new_end_samples, window.new_chunks,
                    ).map(|end_samples| end_samples/16));
                    let mut observation = None;
                    for (index, probability) in new_scores.iter().enumerate() {
                        let end_samples = window.first_new_end_samples
                            + (index * PresenceAudioBuffer::CHUNK_SAMPLES) as u64;
                        let end_ms = end_samples / 16;
                        let state = fuse_presence(*probability, identity.map(|identity| identity.endpoint));
                        if state == PresenceState::Owner {
                            positive_end = Some(end_ms);
                        }
                        observation = Some(PresenceObservation {
                            analyzed_end_ms: end_ms,
                            last_positive_end_ms: positive_end,
                            last_unresolved_end_ms: None,
                            source_history_complete: false,
                            state,
                            observed_at: Instant::now(),
                        });
                    }
                    if let Some(frame) = observation.as_mut() {
                        frame.last_unresolved_end_ms = endpoint_evidence
                            .unresolved_after(positive_end.unwrap_or_default());
                        frame.source_history_complete = true;
                    }
                    drop(source_evidence);
                    if failed.load(Ordering::Acquire) { return Ok(()); }
                    *output.lock() = observation;
                    if let Some(frame) = observation.filter(|frame| frame.analyzed_end_ms >= last_diagnostic_ms + 2_000) {
                        last_diagnostic_ms = frame.analyzed_end_ms;
                        log::info!("[owner-activity] analyzed_ms={} owner_end_ms={:?} state={:?} new_chunks={} compute_ms={} target_probability={:.6}",
                            frame.analyzed_end_ms, frame.last_positive_end_ms, frame.state, window.new_chunks,
                            compute_started.elapsed().as_millis(), new_scores.last().copied().unwrap_or_default());
                    }
                }
                Ok(())
            };
            if let Err(error) = work() {
                failed.store(true, Ordering::Release);
                log::warn!("[owner-activity] unavailable: {error}");
            }
        });
        Self {
            sender: Some(sender),
            latest,
            unavailable,
            timeline,
            raw_activity,
            endpoint_source,
            profile_binding: None,
        }
    }

    pub(crate) fn with_profile_binding(mut self, binding: Option<(String, [u8; 32])>) -> Self {
        self.profile_binding = binding;
        self
    }

    pub(crate) fn endpoint_confirms_other(&self, phrase: &str, key: [u8; 32], start: u64, end: u64) -> bool {
        self.profile_binding.as_ref().is_some_and(|(bound_phrase,bound_key)| bound_phrase == phrase && *bound_key == key)
            && self.endpoint_source.lock().confirms_other(start, end)
    }

    pub(crate) fn submit(&self, pcm: &[u8], source_end_samples: u64) {
        if self.unavailable.load(Ordering::Acquire) {
            return;
        }
        if pcm.len() % 2 != 0
            || self
                .sender
                .as_ref()
                .is_none_or(|sender| sender.try_send((pcm.to_vec(), source_end_samples)).is_err())
        {
            self.unavailable.store(true, Ordering::Release);
            log::warn!("[owner-activity] input unavailable; no negative observation published");
        }
    }

    pub(crate) fn snapshot(&self) -> Option<PresenceObservation> {
        if self.unavailable.load(Ordering::Acquire) {
            None
        } else {
            *self.latest.lock()
        }
    }

    pub(crate) fn speaker_timeline(&self) -> crate::asr::volcengine_speaker_ledger::LocalSpeakerTimeline {
        if self.unavailable.load(Ordering::Acquire) { return Default::default(); }
        self.timeline.lock().clone()
    }

    pub(crate) fn raw_activity_timeline(&self) -> crate::asr::volcengine_speaker_ledger::LocalSpeakerTimeline {
        if self.unavailable.load(Ordering::Acquire) { return Default::default(); }
        self.raw_activity.lock().clone()
    }

    #[cfg(test)]
    pub(crate) fn analysis_unavailable_for_test(&self) -> bool { self.unavailable.load(Ordering::Acquire) }
}

impl Drop for PersonalVadStream {
    fn drop(&mut self) {
        self.unavailable.store(true, Ordering::Release);
        self.sender = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires explicitly selected private history PCM and the existing protected profile; local models only"]
    fn replay_selected_history_model_windows() {
        let root = std::path::PathBuf::from(std::env::var_os("LISTENER_HISTORY_REPLAY").unwrap());
        let protocol: serde_json::Value = serde_json::from_slice(
            &std::fs::read(root.join("protocol.json")).unwrap()).unwrap();
        let loaded = runtime().unwrap();
        let mut cases = Vec::new();
        for case in protocol["cases"].as_array().unwrap() {
            let (reference, verifier, _binding) = crate::speaker_verification::endpoint_presence_references_for_phrase(
                protocol["phrase"].as_str().unwrap()).unwrap();
            let reference = reference.expect("the selected history needs the existing protected reference");
            let mut verifier = verifier.expect("the selected history needs the independent protected identity bank");
            let pcm = std::fs::read(case["pcm"].as_str().unwrap()).unwrap();
            let mut audio = PresenceAudioBuffer::default();
            let mut accepted = 0u64;
            let mut rows = Vec::new();
            for packet in pcm.chunks(PresenceAudioBuffer::CHUNK_SAMPLES * 2) {
                accepted += (packet.len()/2) as u64;
                audio.push(packet, accepted).unwrap();
                while let Some(window) = audio.next_window() {
                    let started = Instant::now();
                    let identity = verifier(&window.pcm).unwrap();
                    let scores = loaded.lock().probabilities(reference.values(), &window.pcm).unwrap();
                    let end_samples = window.first_new_end_samples
                        + ((window.new_chunks-1)*PresenceAudioBuffer::CHUNK_SAMPLES) as u64;
                    let start_ms = (end_samples-(window.pcm.len()/2) as u64)/16;
                    rows.push(serde_json::json!({
                        "startMs":start_ms,"endMs":end_samples/16,"newChunks":window.new_chunks,
                        "identity":format!("{:?}",identity.projection),
                        "endpointIdentity":format!("{:?}",identity.endpoint),"computeMs":started.elapsed().as_millis(),
                        "chunks":scores.iter().enumerate().map(|(index, probability)| serde_json::json!({
                            "startMs":start_ms+index as u64*160,"endMs":start_ms+(index as u64+1)*160,
                            "probability":probability,"state":format!("{:?}",fuse_presence(*probability,Some(identity.endpoint)))
                        })).collect::<Vec<_>>()
                    }));
                }
            }
            cases.push(serde_json::json!({"id":case["id"],"capturedMs":pcm.len()/32,"windows":rows}));
        }
        std::fs::write(root.join("model-windows.json"),serde_json::to_vec_pretty(&serde_json::json!({
            "cases":cases,"cloudCalls":0,"profileModified":false,"vectorsExported":false,
            "schedule":"one source chunk per inference; model and protected references are production code",
            "actualEditor":false
        })).unwrap()).unwrap();
    }

    #[test]
    #[ignore = "requires the selected real premature-stop capture and its current protected registration; entirely local"]
    fn protected_identity_rescues_target_rejections_without_excluding_the_owner() {
        let root = std::path::PathBuf::from(std::env::var_os("LISTENER_PREMATURE_DIAG").unwrap());
        let protocol: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("protocol.json")).unwrap()).unwrap();
        let phrase = protocol["phrase"].as_str().unwrap();
        let reference = crate::speaker_verification::personal_activity_reference_for_phrase(phrase).unwrap().unwrap();
        let mut verifier = crate::speaker_verification::endpoint_identity_verifier_for_phrase(phrase).unwrap().unwrap();
        let pcm = std::fs::read(protocol["capture"].as_str().unwrap()).unwrap();
        let loaded = runtime().unwrap();
        let mut rows = Vec::new();
        let mut rescued = 0;
        for end in protocol["ownerWindowEndsMs"].as_array().unwrap() {
            let end_ms = end.as_u64().unwrap();
            let end_bytes = end_ms as usize * 32;
            let start_bytes = end_bytes.saturating_sub(WINDOW_BYTES);
            let window = &pcm[start_bytes..end_bytes];
            let scores = loaded.lock().probabilities(reference.values(), window).unwrap();
            let probability = *scores.last().unwrap();
            let identity = verifier(window).ok().map(|identity| identity.endpoint).unwrap_or(PresenceState::Unknown);
            let state = fuse_presence(probability, Some(identity));
            if probability < OWNER_ACTIVITY_MIN_PROBABILITY && state == PresenceState::Owner { rescued += 1; }
            rows.push(serde_json::json!({"endMs":end_ms,"targetProbability":probability,"identity":format!("{identity:?}"),"state":format!("{state:?}")}));
        }
        std::fs::write(root.join("protected-window-replay.json"), serde_json::to_vec_pretty(&serde_json::json!({
            "rows":rows,"rescuedOwnerWindows":rescued,"profileModified":false,"vectorsExported":false,"cloudCalls":0,
        })).unwrap()).unwrap();
        assert!(rows.iter().all(|row| row["state"] != "ConfirmedOther"), "this owner was still speaking throughout the prematurely stopped recording");
        assert!(rescued > 0, "independent enrolled identity must preserve this owner's rejected activity windows");
    }

    #[test]
    #[ignore = "requires the explicitly selected premature-stop recording and protected registration; real-time local replay"]
    fn protected_capture_worker_keeps_the_unfinished_owner_turn_recording() {
        let root = std::path::PathBuf::from(std::env::var_os("LISTENER_PREMATURE_DIAG").unwrap());
        let protocol: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("protocol.json")).unwrap()).unwrap();
        let phrase = protocol["phrase"].as_str().unwrap();
        let reference = crate::speaker_verification::personal_activity_reference_for_phrase(phrase).unwrap().unwrap();
        let verifier = crate::speaker_verification::endpoint_identity_verifier_for_phrase(phrase).unwrap();
        let pcm = std::fs::read(protocol["capture"].as_str().unwrap()).unwrap();
        let stream = PersonalVadStream::start(Vec::new(), Some(reference), verifier);
        let start = Instant::now();
        let mut endpoint = super::super::PresenceEndpoint::default();
        let mut rows = Vec::new();
        let mut first_due = None;
        let mut consumed = 0usize;
        for packet in pcm.chunks(3_200) {
            consumed += packet.len();
            stream.submit(packet, (consumed / 2) as u64);
            let due_at = start + std::time::Duration::from_micros(consumed as u64 * 1_000_000 / 32_000);
            std::thread::sleep(due_at.saturating_duration_since(Instant::now()));
            let captured_ms = (consumed / 32) as u64;
            let now = Instant::now();
            if let Some(frame) = stream.snapshot() {
                endpoint.observe(frame);
                if first_due.is_none() && endpoint.due(captured_ms, now, 3_000) { first_due = Some(captured_ms); }
                rows.push(serde_json::json!({"capturedMs":captured_ms,"analyzedMs":frame.analyzed_end_ms,
                    "state":format!("{:?}",frame.state),"ownerEndMs":frame.last_positive_end_ms,
                    "unresolvedEndMs":frame.last_unresolved_end_ms,"lagMs":captured_ms.saturating_sub(frame.analyzed_end_ms)}));
            }
        }
        std::fs::write(root.join("protected-worker-replay.json"),serde_json::to_vec_pretty(&serde_json::json!({
            "firstDueMs":first_due,"rows":rows,"elapsedSeconds":start.elapsed().as_secs_f64(),
            "source":"real prematurely stopped owner recording","actualEditor":false,"cloudCalls":0,
        })).unwrap()).unwrap();
        assert!(first_due.is_none(), "the owner was still speaking when this capture was cut short; no point may authorize STOP");
        assert!(rows.iter().any(|row|row["state"]=="Owner"), "the protected bank must still recognize the owner");
    }

    #[test]
    #[ignore = "requires an explicitly selected private local capture; 55 second real-time replay"]
    fn current_capture_worker_catches_up_and_preserves_the_owner_turn() {
        let root = std::path::PathBuf::from(std::env::var_os("LISTENER_REFERENCE_DIAG").unwrap());
        let protocol: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("protocol.json")).unwrap()).unwrap();
        let reference = protocol["references"].as_array().unwrap().iter()
            .find(|item| item["id"] == "installed_mixed_wake").unwrap();
        let pcm = std::fs::read(protocol["capture"].as_str().unwrap()).unwrap();
        let enrolled = protocol["referenceMode"] == "protected_enrollment";
        let enrollment_reference = if enrolled {
            crate::speaker_verification::personal_activity_reference_for_phrase("开始录音").unwrap()
        } else { None };
        assert!(!enrolled || enrollment_reference.is_some(), "selected production enrollment must be present");
        let stream = PersonalVadStream::start(std::fs::read(reference["path"].as_str().unwrap()).unwrap(), enrollment_reference, crate::speaker_verification::endpoint_identity_verifier_for_phrase("开始录音").unwrap());
        let started = Instant::now();
        let mut endpoint = super::super::PresenceEndpoint::default();
        let mut rows = Vec::new();
        let mut first_due = None;
        let replay_start_ms = protocol["replayStartMs"].as_u64().unwrap_or_default();
        let start_bytes = replay_start_ms as usize * 32;
        assert!(start_bytes < pcm.len() && start_bytes % 2 == 0);
        let mut delivered = [12_000u64, 38_000].map(|end| end <= replay_start_ms);
        let mut consumed = start_bytes;
        for packet in pcm[start_bytes..].chunks(3_200) {
            consumed += packet.len();
            let captured_ms = (consumed / 32) as u64;
            stream.submit(packet, (consumed/2) as u64);
            let due_at = started + std::time::Duration::from_micros(((consumed-start_bytes) as u64)*1_000_000/32_000);
            std::thread::sleep(due_at.saturating_duration_since(Instant::now()));
            let now = Instant::now();
            if let Some(frame) = stream.snapshot() {
                endpoint.observe(frame);
                for (index, audio_ms) in [12_000u64, 38_000].iter().enumerate() {
                    if !delivered[index] && captured_ms >= *audio_ms {
                        endpoint.note_delivery(now);
                        delivered[index] = true;
                    }
                }
                if first_due.is_none() && endpoint.due(captured_ms, now, 3_000) { first_due = Some(captured_ms); }
                rows.push(serde_json::json!({"capturedMs":captured_ms,"analyzedMs":frame.analyzed_end_ms,
                    "ownerEndMs":frame.last_positive_end_ms,"unresolvedEndMs":frame.last_unresolved_end_ms,
                    "state":format!("{:?}", frame.state),"positive":frame.state == PresenceState::Owner,
                    "lagMs":captured_ms.saturating_sub(frame.analyzed_end_ms)}));
            }
        }
        let result = serde_json::json!({"firstDueMs":first_due,"rows":rows,"elapsedSeconds":started.elapsed().as_secs_f64(),"replayStartMs":replay_start_ms,
            "reference":if enrolled { "protected enrollment" } else { "legacy mixed wake" },"actualEditor":false,"noPrivateUpload":true});
        std::fs::write(root.join("production-worker-replay.json"), serde_json::to_vec_pretty(&result).unwrap()).unwrap();
        assert!(first_due.is_some_and(|end| end >= 45_100 && end <= 55_407), "must not stop during this owner's turn, and must release the ongoing video tail");
    }

    #[test]
    #[ignore = "requires selected local uncertainty windows; no capture or cloud"]
    fn diagnose_rejected_enrolled_video_window_sources() {
        let root = std::path::PathBuf::from(std::env::var_os("LISTENER_REFERENCE_DIAG").unwrap());
        let protocol: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("protocol.json")).unwrap()).unwrap();
        let pcm = std::fs::read(protocol["capture"].as_str().unwrap()).unwrap();
        let reference = crate::speaker_verification::personal_activity_reference_for_phrase("开始录音").unwrap().unwrap();
        let mut verifier = crate::speaker_verification::endpoint_identity_verifier_for_phrase("开始录音").unwrap().unwrap();
        let loaded = runtime().unwrap();
        let mut rows = Vec::new();
        for end_ms in [49_760usize, 52_800] {
            let window = &pcm[(end_ms-2_400)*32..end_ms*32];
            let scores = loaded.lock().probabilities(reference.values(), window).unwrap();
            rows.push(serde_json::json!({"endMs":end_ms,"identity":format!("{:?}",verifier(window).unwrap()),
                "scores":scores.iter().enumerate().map(|(index,score)|serde_json::json!({"sourceEndMs":end_ms-2_400+(index+1)*160,"probability":score})).collect::<Vec<_>>()}));
        }
        std::fs::write(root.join("uncertainty-source-scores.json"),serde_json::to_vec_pretty(&rows).unwrap()).unwrap();
    }

    #[test]
    #[ignore = "requires fixed endpoint PCM/ONNX fixtures and the local runtime"]
    fn native_probabilities_match_the_pinned_author_model() {
        let root = std::path::PathBuf::from(std::env::var_os("LISTENER_PVAD_FIXTURES").unwrap());
        let report: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("export-result.json")).unwrap())
                .unwrap();
        let protocol: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("protocol.json")).unwrap()).unwrap();
        let mut native = Runtime::load().unwrap();
        let reference = native
            .reference(&std::fs::read(protocol["referencePcm"].as_str().unwrap()).unwrap())
            .unwrap();
        for case in report["cases"].as_array().unwrap() {
            let id = case["case"].as_str().unwrap();
            let input = protocol["cases"]
                .as_array()
                .unwrap()
                .iter()
                .find(|item| item["id"] == id)
                .unwrap();
            let pcm = std::fs::read(input["path"].as_str().unwrap()).unwrap();
            let end = (case["endMs"].as_f64().unwrap() * 32.0) as usize;
            let start = end.saturating_sub(WINDOW_BYTES);
            let actual = native.probability(&reference, &pcm[start..end]).unwrap();
            let expected = case["torchProbability"].as_f64().unwrap() as f32;
            println!(
                "{id} end_ms={} native={actual:.8} author={expected:.8}",
                end / 32
            );
            assert!((actual - expected).abs() < 0.0001);
            assert_eq!(actual >= OWNER_ACTIVITY_MIN_PROBABILITY, expected >= OWNER_ACTIVITY_MIN_PROBABILITY);
        }
    }
}
