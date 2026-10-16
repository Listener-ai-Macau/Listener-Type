//! Fixed Windows CPU adapter; the endpoint reducer is platform independent.
use super::{fuse_presence, latest_target_source_end, PresenceAudioBuffer, PresenceObservation, PresenceState, PresenceVerifier, OWNER_ACTIVITY_MIN_PROBABILITY};
use ort::{session::Session, value::TensorRef};
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
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
    // Enrollment owns only reference generation, never the streaming detector.
    let values = encode_reference(pcm)?;
    PersonalVadReference::from_bound_values(ENCODER_SHA, crate::speaker_identity_eres::FRONTEND_ID, values)
}

struct Runtime {
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

fn initialize_runtime() -> Result<(), String> {
    let root = crate::persistence::speaker_verification_root().map_err(|e| e.to_string())?;
    let dll = root.join("onnxruntime.dll");
    let initializer = ort::init_from(&dll).map_err(|e| format!("load personal VAD runtime: {e}"))?;
    let _ = initializer.with_name("listener-owner-activity").commit();
    Ok(())
}

fn encode_reference(pcm: &[u8]) -> Result<Vec<f32>, String> {
        // Keep transient generation serialized as the old shared runtime did,
        // without retaining an encoder or blocking streaming detector calls.
        static GENERATION: once_cell::sync::Lazy<Mutex<()>> =
            once_cell::sync::Lazy::new(|| Mutex::new(()));
        let _generation = GENERATION.lock();
        let (frames, values) = crate::speaker_identity_eres::fbank(pcm)?;
        initialize_runtime()?;
        // The caller retains the 192 values, not a native model or borrowed
        // output. Drop the encoder on success and on every error path.
        let mut encoder = load_session(&model_path(ENCODER)?, ENCODER_SHA)?;
        let input = TensorRef::from_array_view(([1usize, frames, 80usize], values.as_slice()))
            .map_err(|e| e.to_string())?;
        let output = encoder
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

impl Runtime {
    fn load() -> Result<Self, String> {
        initialize_runtime()?;
        Ok(Self { detector: load_session(&model_path(DETECTOR)?, DETECTOR_SHA)? })
    }

    #[cfg(test)]
    fn reference(&mut self, pcm: &[u8]) -> Result<Vec<f32>, String> {
        encode_reference(pcm)
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
    input: Arc<super::input_queue::PresenceInputQueue>,
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
        let input = Arc::new(super::input_queue::PresenceInputQueue::default());
        let worker_input = Arc::clone(&input);
        let latest = Arc::new(Mutex::new(None::<PresenceObservation>));
        let unavailable = Arc::new(AtomicBool::new(false));
        let timeline = Arc::new(Mutex::new(crate::asr::volcengine_speaker_ledger::LocalSpeakerTimeline::default()));
        let source_timeline = Arc::clone(&timeline);
        let raw_activity = Arc::new(Mutex::new(crate::asr::volcengine_speaker_ledger::LocalSpeakerTimeline::default()));
        let raw_output = Arc::clone(&raw_activity);
        let endpoint_source = Arc::new(Mutex::new(super::EndpointSourceTimeline::default()));
        let endpoint_output = Arc::clone(&endpoint_source);
        let output = Arc::clone(&latest);
        let failed = Arc::clone(&unavailable);
        let worker_lifetime = input.worker_lifetime();
        tauri::async_runtime::spawn_blocking(move || {
            let _worker_lifetime = worker_lifetime;
            #[cfg(test)]
            eprintln!("[saved-presence] worker entered");
            let work = || -> Result<(), String> {
                let source = if enrolled_reference.is_some() { "three_sample_enrollment" } else { "legacy_verified_wake" };
                let reference = match enrolled_reference {
                    Some(reference) => reference.0,
                    None => encode_reference(&reference_pcm)?,
                };
                let loaded = runtime()?;
                #[cfg(test)]
                eprintln!("[saved-presence] runtime loaded");
                log::info!("[owner-activity] reference ready source={source} verified_wake_pcm_ms={} output=all_chunks", reference_pcm.len()/32);
                let mut audio = PresenceAudioBuffer::default();
                let mut positive_end = None;
                let mut last_diagnostic_ms = 0;
                let mut generation = 0;
                loop {
                    if failed.load(Ordering::Acquire) {
                        return Ok(());
                    }
                    // Take at most one model window, so backlog stays inside
                    // the byte-bounded mailbox rather than migrating unbounded
                    // into this worker's private pending buffer.
                    if let Some(batch) = worker_input.take_limit(!audio.has_complete_chunk(),
                        PresenceAudioBuffer::MAX_NEW_PCM_BYTES) {
                        if generation != batch.generation {
                            audio = PresenceAudioBuffer::default();
                            positive_end = output.lock().as_ref().and_then(|frame| frame.last_positive_end_ms);
                            generation = batch.generation;
                        }
                        audio.push(&batch.pcm, batch.end_samples)?;
                    } else if !audio.has_complete_chunk() {
                        break;
                    }
                    // Retain the initial origin hold, but never mint a new
                    // positive identity boundary from an analysis overrun.
                    if positive_end.is_none() && generation == 1 {
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
                    let scores = match scores {
                        Ok(scores) => scores,
                        Err(error) if worker_input.with_current(generation,
                            || failed.store(true, Ordering::Release)).is_some() => return Err(error),
                        Err(_) => continue, // retired work cannot disable the new generation
                    };
                    #[cfg(test)]
                    eprintln!("[saved-presence] window complete");
                    let publication = worker_input.with_current(generation, || {
                        if failed.load(Ordering::Acquire) { return Ok(None); }
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
                        *output.lock() = observation;
                        Ok::<_, String>(observation)
                    });
                    let Some(observation) = publication else { continue; };
                    let observation = observation?;
                    if let Some(frame) = observation.filter(|frame| frame.analyzed_end_ms >= last_diagnostic_ms + 2_000) {
                        last_diagnostic_ms = frame.analyzed_end_ms;
                        log::info!("[owner-activity] analyzed_ms={} owner_end_ms={:?} state={:?} new_chunks={} compute_ms={} target_probability={:.6}",
                            frame.analyzed_end_ms, frame.last_positive_end_ms, frame.state, window.new_chunks,
                            compute_started.elapsed().as_millis(), scores.last().copied().unwrap_or_default());
                    }
                }
                Ok(())
            };
            if let Err(error) = work() {
                failed.store(true, Ordering::Release);
                worker_input.close();
                log::warn!("[owner-activity] unavailable: {error}");
            }
        });
        Self {
            input,
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
            && !self.unavailable.load(Ordering::Acquire)
            && self.input.with_open(|| self.endpoint_source.lock().confirms_other(start, end)).unwrap_or(false)
    }

    pub(crate) fn submit(&self, pcm: &[u8], source_end_samples: u64) {
        if self.unavailable.load(Ordering::Acquire) { return; }
        let admission = self.input.submit(pcm, source_end_samples, |new_origin_samples| {
            let gap_end_ms = new_origin_samples / 16;
            let mut latest = self.latest.lock();
            let gap_start_ms = latest.as_ref().map(|frame| frame.analyzed_end_ms).unwrap_or(0);
            if gap_start_ms < gap_end_ms {
                let gap = crate::asr::volcengine_speaker_ledger::SpeakerSpan {
                    start_ms: gap_start_ms, end_ms: gap_end_ms,
                    kind: crate::asr::volcengine_speaker_ledger::EvidenceKind::Unknown,
                };
                self.timeline.lock().note_presence(gap);
                self.raw_activity.lock().note_presence(gap);
                self.endpoint_source.lock().note_unobserved_gap(gap_start_ms, gap_end_ms);
            }
            if let Some(frame) = latest.as_mut() {
                frame.state = PresenceState::Unknown;
                frame.source_history_complete = false;
                frame.last_unresolved_end_ms = frame.last_unresolved_end_ms.max(Some(gap_end_ms));
                frame.observed_at = Instant::now();
            }
        });
        match admission {
            Ok(admission) if admission.rebased => log::warn!(
                "[owner-activity] analysis rebased generation={} source_end_samples={source_end_samples}; skipped source stays Unknown",
                admission.generation),
            Ok(_) => {},
            Err(error) => {
                self.unavailable.store(true, Ordering::Release);
                self.input.close();
                log::warn!("[owner-activity] input unavailable: {error}; no negative observation published");
            }
        }
    }

    pub(crate) fn snapshot(&self) -> Option<PresenceObservation> {
        if self.unavailable.load(Ordering::Acquire) {
            None
        } else {
            self.input.with_open(|| *self.latest.lock()).flatten()
        }
    }

    pub(crate) fn speaker_timeline(&self) -> crate::asr::volcengine_speaker_ledger::LocalSpeakerTimeline {
        if self.unavailable.load(Ordering::Acquire) { return Default::default(); }
        self.input.with_open(|| self.timeline.lock().clone()).unwrap_or_default()
    }

    pub(crate) fn raw_activity_timeline(&self) -> crate::asr::volcengine_speaker_ledger::LocalSpeakerTimeline {
        if self.unavailable.load(Ordering::Acquire) { return Default::default(); }
        self.input.with_open(|| self.raw_activity.lock().clone()).unwrap_or_default()
    }

    #[cfg(test)]
    pub(crate) fn analysis_unavailable_for_test(&self) -> bool {
        self.unavailable.load(Ordering::Acquire) || self.input.with_open(|| ()).is_none()
    }
}

impl Drop for PersonalVadStream {
    fn drop(&mut self) {
        self.unavailable.store(true, Ordering::Release);
        self.input.close();
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
    #[test]
    #[ignore = "requires the archived startup PCM and existing protected profile; no device/cloud"]
    fn saved_startup_burst_keeps_personal_activity_source_alive() {
        let root = std::path::PathBuf::from(std::env::var_os("LISTENER_PERSONAL_QUEUE_REPLAY").unwrap());
        let pcm = std::fs::read(root.join("capture.pcm")).unwrap();
        assert!(pcm.len() >= 56_400 * 32);
        let full_capture = std::env::var_os("LISTENER_PERSONAL_QUEUE_FULL_SOURCE").is_some();
        let input_bytes = if full_capture { pcm.len() } else { 56_400 * 32 };
        let captured_ms = (input_bytes / 32) as u64;
        let expected_analyzed_ms = (input_bytes / 2 / PresenceAudioBuffer::CHUNK_SAMPLES
            * PresenceAudioBuffer::CHUNK_SAMPLES / 16) as u64;
        let continuation_speed = if full_capture { 5 } else { 1 };
        let (reference, verifier, binding) = crate::speaker_verification::endpoint_presence_references_for_phrase("开始录音").unwrap();
        let stream = PersonalVadStream::start(Vec::new(), reference, verifier).with_profile_binding(binding);
        let began = Instant::now();
        let mut snapshots = Vec::new();
        let mut consumed = 0;
        for packet in pcm[..54_000 * 32].chunks(3_200) {
            consumed += packet.len();
            stream.submit(packet, (consumed/2) as u64);
        }
        assert!(!stream.analysis_unavailable_for_test(), "startup is a byte-bounded backlog, not fatal input");
        // Keep the original startup burst, then feed fresh audio at its real
        // 100ms cadence while inference is still catching up. Do not wait for
        // analysis between packets or before admitting this continuation.
        let continuation_started = Instant::now();
        let mut continuation_bytes = 0;
        let mut endpoint = super::super::PresenceEndpoint::default();
        let mut max_analyzed_during_continuation_ms = 0;
        for packet in pcm[54_000 * 32..input_bytes].chunks(3_200) {
            consumed += packet.len();
            continuation_bytes += packet.len();
            stream.submit(packet, (consumed/2) as u64);
            if let Some(frame) = stream.snapshot() {
                endpoint.observe(frame);
                max_analyzed_during_continuation_ms = max_analyzed_during_continuation_ms.max(frame.analyzed_end_ms);
                if (consumed/32) as u64 > frame.analyzed_end_ms + 250 {
                    assert!(!endpoint.due((consumed/32) as u64, Instant::now(), 3_000),
                        "fresh result observation cannot turn old-source backlog into absence now");
                }
            }
            let due = continuation_started + std::time::Duration::from_micros(
                continuation_bytes as u64 * 1_000_000 / (32_000 * continuation_speed));
            std::thread::sleep(due.saturating_duration_since(Instant::now()));
        }
        loop {
            assert!(began.elapsed() < std::time::Duration::from_secs(75), "source worker did not catch up");
            assert!(!stream.analysis_unavailable_for_test());
            if let Some(frame) = stream.snapshot() {
                if snapshots.last().and_then(|row: &serde_json::Value| row["analyzedMs"].as_u64()) != Some(frame.analyzed_end_ms) {
                    snapshots.push(serde_json::json!({"analyzedMs":frame.analyzed_end_ms,
                        "ownerEndMs":frame.last_positive_end_ms,"unresolvedMs":frame.last_unresolved_end_ms,
                        "historyComplete":frame.source_history_complete,"state":format!("{:?}",frame.state)}));
                }
                if frame.analyzed_end_ms >= expected_analyzed_ms { break; }
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let latest = stream.snapshot().unwrap();
        assert_eq!(latest.analyzed_end_ms, expected_analyzed_ms);
        assert!(latest.source_history_complete);
        let mut native_projection = serde_json::Value::Null;
        if let Some(path) = std::env::var_os("LISTENER_PERSONAL_QUEUE_SOURCE_TRACE") {
            use crate::asr::volcengine::{VolcengineCredentials, VolcengineStreamingASR};
            use crate::asr::volcengine_native_identity::{NativeSourceIdentity, detector_observed_without_owner};
            let trace: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
            let entries = trace["trace"]["entries"].as_array().unwrap();
            let frames: Vec<_> = entries.iter().filter(|entry| entry["stage"] == "normalized_result"
                && entry["provider_result"].is_object()).collect();
            let mut asr = VolcengineStreamingASR::new(VolcengineCredentials {
                app_id: "offline".into(), access_token: "offline".into(), resource_id: "offline".into(),
            }, Vec::new()).with_owner_activity_only(true);
            asr.enable_diagnostic_trace();
            asr.note_accepted_wake_phrase("开始录音");
            let local = stream.speaker_timeline();
            let raw = stream.raw_activity_timeline();
            let mut analyzer = NativeSourceIdentity::default();
            let mut rows = Vec::new();
            let mut skipped_unprocessed_frames = 0;
            assert!(!detector_observed_without_owner(&raw,latest.analyzed_end_ms,captured_ms),
                "the final unprocessed partial chunk must remain unobserved");
            for frame in frames {
                if frame["provider_result"]["utterances"].as_array().is_some_and(|utterances|
                    utterances.iter().any(|row| row["end_time"].as_u64().is_some_and(|end| end > latest.analyzed_end_ms))) {
                    skipped_unprocessed_frames += 1;
                    continue;
                }
                let native = analyzer.analyze("开始录音", &frame["provider_result"], &pcm, &local, raw.clone());
                for &(start,end,_) in &native.rows {
                    if end > latest.analyzed_end_ms {
                        assert!(!detector_observed_without_owner(&native.raw_activity,start,end),
                            "newly available prefix cannot grant absent evidence to the unprocessed capture tail");
                    }
                }
                rows.push(serde_json::json!({"originalFrame":frame["frame"],
                    "qualifiedReference":native.qualified_reference,
                    "sourceRows":native.rows.iter().map(|(start,end,kind)|
                        serde_json::json!([start,end,format!("{:?}",kind)])).collect::<Vec<_>>()}));
                asr.install_native_word_identity_for_test(native);
                // The original recording was cancelled: replay its actual
                // responses without inventing an EOF or successful final text.
                assert!(!frame["final_frame"].as_bool().unwrap_or(false));
                let _ = asr.apply_saved_native_result_with_local_timeline_for_test(
                    &frame["provider_result"], false, &local);
            }
            let candidate_trace = asr.take_diagnostic_trace();
            native_projection = serde_json::json!({"frames":rows,
                "skippedUnprocessedFrames":skipped_unprocessed_frames,
                "trace":candidate_trace,"actualEof":false,"actualEditor":false,
                "scope":"post-catchup source/ordinary native projection; no live timing or ground-truth word-removal claim"});
        }
        let result = serde_json::json!({"startupBurstMs":54_000,"capturedMs":captured_ms,
            "analyzedMs":latest.analyzed_end_ms,"available":!stream.analysis_unavailable_for_test(),
            "continuationPaced":continuation_speed==1,"continuationSpeed":continuation_speed,"fullCapture":full_capture,"maxAnalyzedDuringContinuationMs":max_analyzed_during_continuation_ms,
            "elapsedSeconds":began.elapsed().as_secs_f64(),"snapshots":snapshots,
            "sourceTimeline":format!("{:?}",stream.speaker_timeline()),
            "rawActivityTimeline":format!("{:?}",stream.raw_activity_timeline()),
            "actualEditor":false,"actualStop":false,"newModel":false,"nativeProjection":native_projection});
        std::fs::write(root.join(if full_capture { "full-capture-worker-result.json" } else { "personal-worker-startup-result.json" }),serde_json::to_vec_pretty(&result).unwrap()).unwrap();
    }


    #[test]
    #[ignore = "complete archived source grade diagnosis, existing models; no queue pacing/live timing, editor, device, cloud, profile writes or STOP"]
    fn complete_saved_source_distinguishes_word_other_from_endpoint_absence() {
        use crate::asr::volcengine_speaker_ledger::LocalSpeakerTimeline;
        use crate::asr::volcengine_native_identity::NativeSourceIdentity;
        use crate::asr::volcengine::{VolcengineCredentials,VolcengineStreamingASR};
        use super::super::{EndpointSourceTimeline,publish_scored_window};
        let _=env_logger::builder().is_test(true).filter_level(log::LevelFilter::Info).try_init();
        let manifest:serde_json::Value=serde_json::from_slice(&std::fs::read(
            std::env::var("LISTENER_COMPLETE_SOURCE_INPUT").unwrap()).unwrap()).unwrap();
        let phrase=manifest["phrase"].as_str().unwrap();
        assert!(crate::speaker_verification::has_bound_template_without_mutation(phrase));
        let mut reports=Vec::new();let began=Instant::now();
        for case in manifest["cases"].as_array().unwrap() {
            let pcm=std::fs::read(case["pcm"].as_str().unwrap()).unwrap();
            assert_eq!(format!("{:x}",Sha256::digest(&pcm)),case["sha256"].as_str().unwrap());
            let (reference,mut verifier,_)=crate::speaker_verification::endpoint_presence_references_for_phrase(phrase).unwrap();
            let reference=reference.unwrap().values().to_vec();let loaded=runtime().unwrap();
            let mut audio=PresenceAudioBuffer::default();let mut consumed=0;
            let mut raw=LocalSpeakerTimeline::default();let mut local=LocalSpeakerTimeline::default();
            let mut endpoint=EndpointSourceTimeline::default();let mut rows=Vec::new();
            for packet in pcm.chunks(PresenceAudioBuffer::MAX_NEW_PCM_BYTES) {
                consumed+=packet.len();audio.push(packet,(consumed/2) as u64).unwrap();
                while let Some(window)=audio.next_window() {
                    assert!(began.elapsed()<std::time::Duration::from_secs(78));
                    let (scores,identity)=std::thread::scope(|scope| {
                        let check=verifier.as_mut().unwrap();
                        let identity=scope.spawn(|| check(&window.pcm).ok());
                        let scores=loaded.lock().probabilities(&reference,&window.pcm).unwrap();
                        (scores,identity.join().unwrap())
                    });
                    publish_scored_window(&mut raw,&scores,window.first_new_end_samples,window.new_chunks,None).unwrap();
                    publish_scored_window(&mut local,&scores,window.first_new_end_samples,window.new_chunks,identity.map(|v|v.projection)).unwrap();
                    endpoint.note_scored_window(&scores,window.first_new_end_samples,window.new_chunks,identity).unwrap();
                    let start=(window.first_new_end_samples/16)-160;
                    let end=start+window.new_chunks as u64*160;
                    rows.push(serde_json::json!({"startMs":start,"endMs":end,
                        "projectionIdentity":format!("{:?}",identity.map(|v|v.projection)),"endpointIdentity":format!("{:?}",identity.map(|v|v.endpoint)),
                        "rawOwnerChunks":scores.iter().filter(|x|**x>=OWNER_ACTIVITY_MIN_PROBABILITY).count(),
                        "maxRawProbability":scores.iter().copied().fold(0f32,f32::max),
                        "endpointConfirmsWholeWindowOther":endpoint.confirms_other(start,end)}));
                }
            }
            let analyzed=rows.last().unwrap()["endMs"].as_u64().unwrap();
            assert_eq!(analyzed,(pcm.len()/5120*160) as u64);
            let mut native_frames=Vec::new();let mut skipped=0;
            if let Some(path)=case["trace"].as_str() {
                let trace:serde_json::Value=serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
                let mut analyzer=NativeSourceIdentity::default();
                let mut asr=VolcengineStreamingASR::new(VolcengineCredentials{app_id:"offline".into(),access_token:"offline".into(),resource_id:"offline".into()},Vec::new()).with_owner_activity_only(true);
                asr.note_accepted_wake_phrase(phrase);
                for entry in trace["trace"]["entries"].as_array().unwrap().iter().filter(|e|e["stage"]=="normalized_result" && e["provider_result"].is_object()) {
                    let result=&entry["provider_result"];
                    if result["utterances"].as_array().is_some_and(|rows|rows.iter().any(|r|r["end_time"].as_u64().is_some_and(|end|end>analyzed))) {skipped+=1;continue;}
                    let native=analyzer.analyze(phrase,result,&pcm,&local,raw.clone());
                    let qualified=native.qualified_reference;
                    let source_rows=native.rows.iter().map(|(a,b,kind)|serde_json::json!({"startMs":a,"endMs":b,
                        "wordIdentity":format!("{:?}",kind),"endpointConfirmsOther":endpoint.confirms_other(*a,*b)})).collect::<Vec<_>>();
                    asr.install_native_word_identity_for_test(native);
                    // Preserve only responses actually received; do not mint EOF.
                    let output=asr.apply_saved_native_result_with_local_timeline_for_test(result,entry["final_frame"]==true,&local);
                    native_frames.push(serde_json::json!({"frame":entry["frame"],"qualifiedReference":qualified,"sourceRows":source_rows,
                        "originalFinalFrame":entry["final_frame"],"projectedText":output.map(|o|o.text)}));
                }
            }
            reports.push(serde_json::json!({"id":case["id"],"pcmSha256":case["sha256"],"capturedMs":pcm.len()/32,
                "analyzedMs":analyzed,"unprocessedTailMs":pcm.len()/32-analyzed as usize,"windows":rows,"nativeFrames":native_frames,"skippedUnprocessedFrames":skipped}));
            std::fs::write(std::env::var("LISTENER_COMPLETE_SOURCE_OUTPUT").unwrap(),serde_json::to_vec_pretty(&serde_json::json!({"partial":true,"cases":reports})).unwrap()).unwrap();
        }
        std::fs::write(std::env::var("LISTENER_COMPLETE_SOURCE_OUTPUT").unwrap(),serde_json::to_vec_pretty(&serde_json::json!({
            "schema":"listener.complete-source-grade-diagnosis.v1","cases":reports,"elapsedSeconds":began.elapsed().as_secs_f64(),
            "scope":"Exact production model, source buffer, grades, timelines and native projection with complete prior-source availability and fixed 2.4s batching. No queue/load timing or live response-readiness claim; no fabricated source labels, receipts, EOF, editor or STOP.",
            "newWeights":false,"cloudCalls":0,"actualEditor":false,"actualStop":false})).unwrap()).unwrap();
    }


    #[test]
    #[ignore = "real-paced archived unresolved phone tail through actual stream/endpoint source grades; no cloud, editor receipts or physical STOP"]
    fn saved_phone_tail_gets_realtime_rolling_source_evidence() {
        let _=env_logger::builder().is_test(true).filter_level(log::LevelFilter::Info).try_init();
        let fixture:serde_json::Value=serde_json::from_slice(&std::fs::read(
            std::env::var("LISTENER_REALTIME_GRADE_INPUT").unwrap()).unwrap()).unwrap();
        let original=std::fs::read(fixture["pcm"].as_str().unwrap()).unwrap();
        assert_eq!(format!("{:x}",Sha256::digest(&original)),fixture["sha256"].as_str().unwrap());
        let phrase=fixture["phrase"].as_str().unwrap();
        assert!(crate::speaker_verification::has_bound_template_without_mutation(phrase));
        // The real wake already loads this scorer; no reference is learned,
        // no observation from this call grants an Owner/source permission.
        let bootstrap=crate::speaker_verification::native_word_profile_for_phrase(phrase).unwrap().unwrap();
        let _=bootstrap.observe(&original[..1440*32]).unwrap();
        let (reference,verifier,binding)=crate::speaker_verification::endpoint_presence_references_for_phrase(phrase).unwrap();
        let stream=PersonalVadStream::start(Vec::new(),reference,verifier).with_profile_binding(binding);
        let pcm=&original[..fixture["through_ms"].as_u64().unwrap() as usize*32];
        let began=Instant::now();let mut consumed=0;let mut rows=Vec::new();
        let mut endpoint=super::super::PresenceEndpoint::default();let mut first_source_due=None;
        for packet in pcm.chunks(3200) {
            consumed+=packet.len();let captured_ms=(consumed/32) as u64;
            stream.submit(packet,(consumed/2) as u64);
            assert!(!stream.analysis_unavailable_for_test());
            if let Some(frame)=stream.snapshot() {
                endpoint.observe(frame);
                let now=Instant::now();let due=endpoint.due(captured_ms,now,3000);
                if due && first_source_due.is_none(){first_source_due=Some(captured_ms);}
                let last3000_other=frame.analyzed_end_ms.checked_sub(3000).is_some_and(|a|
                    stream.endpoint_source.lock().confirms_other(a,frame.analyzed_end_ms));
                if rows.last().and_then(|r:&serde_json::Value|r["analyzedMs"].as_u64())!=Some(frame.analyzed_end_ms) {
                    rows.push(serde_json::json!({"capturedMs":captured_ms,"wallElapsedMs":began.elapsed().as_millis(),
                        "analyzedMs":frame.analyzed_end_ms,"ownerEndMs":frame.last_positive_end_ms,
                        "unresolvedMs":frame.last_unresolved_end_ms,"historyComplete":frame.source_history_complete,
                        "state":format!("{:?}",frame.state),"last3000MsEndpointOther":last3000_other,
                        "sourceOnlyDueNoDeliveryClaim":due}));
                }
            }
            let due=began+std::time::Duration::from_millis(captured_ms);
            std::thread::sleep(due.saturating_duration_since(Instant::now()));
        }
        let captured_ms=(pcm.len()/32) as u64;let expected=(pcm.len()/5120*160) as u64;
        loop {
            assert!(began.elapsed()<std::time::Duration::from_secs(40));
            assert!(!stream.analysis_unavailable_for_test());
            if stream.snapshot().is_some_and(|frame|frame.analyzed_end_ms>=expected){break;}
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let last=stream.snapshot().unwrap();
        assert_eq!(last.analyzed_end_ms,expected);
        std::fs::write(std::env::var("LISTENER_REALTIME_GRADE_OUTPUT").unwrap(),serde_json::to_vec_pretty(&serde_json::json!({
            "schema":"listener.real-paced-source-grade.v1","sourceSession":fixture["session"],"originalSha256":fixture["sha256"],
            "prefixSha256":format!("{:x}",Sha256::digest(pcm)),"capturedMs":captured_ms,"analyzedMs":last.analyzed_end_ms,
            "firstSourceOnlyDueMs":first_source_due,"observations":rows,"elapsedSeconds":began.elapsed().as_secs_f64(),
            "rawTimeline":format!("{:?}",stream.raw_activity_timeline()),"sourceTimeline":format!("{:?}",stream.speaker_timeline()),
            "endpointTimeline":format!("{:?}",stream.endpoint_source.lock()),
            "scope":"27s prefix stops amid actual saved phone material, real100ms ingress pacing, existing immutable enrollment references and production stream. PresenceEndpoint queried without fabricated editor receipts: source eligibility only, not successful product delivery/STOP.",
            "actualEditor":false,"actualStop":false,"newWeights":false,"cloudCalls":0})).unwrap()).unwrap();
    }

    #[test]
    #[ignore="Explicit same-model background-reference diagnostic; no product or editor activity"]
    fn existing_nomo_background_condition_is_not_owner_absence() {
        use serde_json::{Value,json};
        let path=std::path::PathBuf::from(std::env::var("LISTENER_EXISTING_NOMO_BACKGROUND_FIXTURE").unwrap());
        let fixture:Value=serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let phrase=fixture["phrase"].as_str().unwrap();assert!(crate::speaker_verification::has_bound_template_without_mutation(phrase));
        let (owner,mut verifier,_)=crate::speaker_verification::endpoint_presence_references_for_phrase(phrase).unwrap();let owner=owner.unwrap();
        let read_source=|row:&Value| {
            let bytes=std::fs::read(row["pcm"].as_str().unwrap()).unwrap();
            assert_eq!(format!("{:x}",Sha256::digest(&bytes)),row["pcmSha256"].as_str().unwrap());
            bytes[row["startMs"].as_u64().unwrap() as usize*32..row["endMs"].as_u64().unwrap() as usize*32].to_vec()
        };
        let mut runtime=Runtime::load().unwrap();
        let background=runtime.reference(&read_source(&fixture["backgroundReference"])).unwrap();
        let mut rows=Vec::new();let began=Instant::now();
        for row in fixture["inputs"].as_array().unwrap() {
            let pcm=read_source(row);assert_eq!(pcm.len(),2400*32);
            let began=Instant::now();let owner_scores=runtime.probabilities(owner.values(),&pcm).unwrap();
            let background_scores=runtime.probabilities(&background,&pcm).unwrap();
            let identity=verifier.as_mut().map(|v|v(&pcm).unwrap());
            let owner_last=*owner_scores.last().unwrap();let background_last=*background_scores.last().unwrap();
            let raw_background_only=owner_last<OWNER_ACTIVITY_MIN_PROBABILITY && background_last>=OWNER_ACTIVITY_MIN_PROBABILITY;
            let guarded_background_only=raw_background_only && identity.is_some_and(|v|v.projection==PresenceState::ConfirmedOther);
            rows.push(json!({"id":row["id"],"rangeMs":[row["startMs"],row["endMs"]],"assessmentOnly":row["assessmentOnly"],
                "ownerProbabilities":owner_scores,"backgroundProbabilities":background_scores,
                "currentIdentityProjection":format!("{:?}",identity.map(|v|v.projection)),"currentIdentityEndpoint":format!("{:?}",identity.map(|v|v.endpoint)),
                "rawBackgroundOnlyAtLastChunk":raw_background_only,"coarseBankGuardedBackgroundOnlyAtLastChunk":guarded_background_only,
                "sameSourceOwnerRescueStillWins":owner_last>=OWNER_ACTIVITY_MIN_PROBABILITY || identity.is_some_and(|v|v.projection==PresenceState::Owner),
                "computeMs":began.elapsed().as_secs_f64()*1000.0}));
        }
        std::fs::write(path.parent().unwrap().join("result.json"),serde_json::to_vec_pretty(&json!({
            "scope":fixture["scope"],"modelSha256":DETECTOR_SHA,"rows":rows,"sourceComputeSeconds":began.elapsed().as_secs_f64(),
            "newModel":false,"newWeights":false,"referencesExported":false,"profileWrites":0,"cloudCalls":0,"actualEditor":false,"actualStop":false,
        })).unwrap()).unwrap();
    }

}
