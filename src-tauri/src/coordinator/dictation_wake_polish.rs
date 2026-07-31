// Wake candidate, speaker gate, streaming polish helpers.
// Included into `coordinator::dictation` via `include!`.

impl EmbeddedAudioDictationSession {
    fn consume_streaming_pcm(
        &mut self,
        inner: &Arc<Inner>,
        pcm: &[u8],
        raw_input_level_percent: Option<u8>,
    ) -> Result<(), String> {
        if pcm.is_empty() {
            return Ok(());
        }
        if pcm.len() % 2 != 0 {
            return Err("嵌入式音频 PCM chunk 长度不是 16-bit 对齐".to_string());
        }

        if embedded_audio_stop_feedback_latched(inner) {
            if !embedded_audio_streaming_session_accepts_pcm(inner, self.session_id) {
                log::debug!(
                    "[coord] embedded audio streaming PCM ignored for inactive dictation session ({})",
                    self.session_id
                );
                return Ok(());
            }
        } else {
            if !emit_embedded_audio_pcm_capsule_if_active(
                inner,
                self.session_id,
                CapsuleState::Recording,
                embedded_pcm_capsule_level(pcm, raw_input_level_percent),
            ) {
                log::debug!(
                    "[coord] embedded audio streaming PCM ignored for inactive dictation session ({})",
                    self.session_id
                );
                return Ok(());
            }
        }

        if let Some(archive_pcm) = self.archive_pcm.as_mut() {
            archive_pcm.extend_from_slice(pcm);
        }

        self.streamed_pcm_bytes += pcm.len();
        self.streaming_pcm_buffer.extend_from_slice(pcm);
        self.consume_ready_streaming_pcm_blocks();
        Ok(())
    }

    fn flush_streaming_pcm(&mut self) {
        if self.streaming_pcm_buffer.is_empty() {
            return;
        }

        let trailing_pcm = std::mem::take(&mut self.streaming_pcm_buffer);
        self.consume_prepared_streaming_pcm(&trailing_pcm);
    }

    fn consume_ready_streaming_pcm_blocks(&mut self) {
        let ready_bytes = self.streaming_pcm_buffer.len() / EMBEDDED_AUDIO_FEED_CHUNK_BYTES
            * EMBEDDED_AUDIO_FEED_CHUNK_BYTES;
        if ready_bytes == 0 {
            return;
        }

        let trailing_pcm = self.streaming_pcm_buffer.split_off(ready_bytes);
        let ready_pcm = std::mem::replace(&mut self.streaming_pcm_buffer, trailing_pcm);
        for pcm_block in ready_pcm.chunks(EMBEDDED_AUDIO_FEED_CHUNK_BYTES) {
            self.consume_prepared_streaming_pcm(pcm_block);
        }
    }

    fn consume_prepared_streaming_pcm(&mut self, pcm: &[u8]) {
        let source_pcm_offset_ms = (self.normalized_pcm_bytes as u64) / 32;
        let (asr_pcm, gain_stats) = self.prepare_streaming_pcm_for_asr(pcm);
        if self.active_asr == "volcengine"
            && embedded_streaming_chunk_has_speech_energy(
                gain_stats.rms_before,
                gain_stats.peak_before,
            )
        {
            self.streaming_agc
                .first_voiced_pcm_ms
                .get_or_insert(source_pcm_offset_ms);
        }

        // 改A: track sustained trailing silence AFTER the body has started so the
        // caller can request a host-initiated device stop early. Leading silence
        // (before the user speaks the dictation body) and post-stop tails never
        // count toward the threshold.
        let chunk_ms = (pcm.len() / 32) as u64;
        let (signal_rms, signal_peak) = embedded_pcm_streaming_agc_signal_level(pcm);
        if embedded_streaming_chunk_has_speech_energy(signal_rms, signal_peak) {
            self.proactive_stop_body_started = true;
            self.proactive_stop_silence_ms = 0;
        } else if self.proactive_stop_body_started {
            self.proactive_stop_silence_ms = self
                .proactive_stop_silence_ms
                .saturating_add(chunk_ms);
        }

        self.normalized_pcm_bytes += asr_pcm.len();
        self.consumer.consume_pcm_chunk(&asr_pcm);
    }

    fn prepare_streaming_pcm_for_asr(&mut self, pcm: &[u8]) -> (Vec<u8>, EmbeddedPcmGainStats) {
        if self.active_asr == "volcengine" {
            // The buffer is bounded to one established provider feed interval.
            return normalize_embedded_streaming_pcm_for_asr(pcm, &mut self.streaming_agc);
        }

        normalize_embedded_pcm_for_asr(pcm)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BufferedSpeakerCandidateKind {
    Enrollment,
    Verification,
    Rejected,
}

const HIDDEN_AUTOMATIC_CANDIDATE_NONE: u8 = 0;
const HIDDEN_AUTOMATIC_CANDIDATE_ACTIVE: u8 = 1;
const HIDDEN_AUTOMATIC_CANDIDATE_PROMOTION_REQUESTED: u8 = 2;
static HIDDEN_AUTOMATIC_CANDIDATE_STATE: AtomicU8 = AtomicU8::new(HIDDEN_AUTOMATIC_CANDIDATE_NONE);
/// Device-key Start pressed while a hidden VA candidate was not ACTIVE yet (KWS init
/// race or host lag). When the candidate becomes ACTIVE, auto-request promotion.
static DEVICE_KEY_DICTATION_TAKEOVER_PENDING: AtomicBool = AtomicBool::new(false);
static WAKE_DIAGNOSTIC_CAPTURE_COUNT: AtomicUsize = AtomicUsize::new(0);
static WAKE_DIAGNOSTIC_CLEANUP_RUNNING: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone)]
struct WakeDiagnosticRetentionEntry {
    path: std::path::PathBuf,
    modified: std::time::SystemTime,
    bytes: u64,
}

fn wake_diagnostic_retention_plan(
    mut entries: Vec<WakeDiagnosticRetentionEntry>,
    now: std::time::SystemTime,
    max_age: Duration,
    max_files: usize,
    max_bytes: u64,
) -> Vec<std::path::PathBuf> {
    entries.sort_by_key(|entry| {
        entry
            .modified
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
    });

    let mut remove = Vec::new();
    let mut retained = Vec::with_capacity(entries.len());
    for entry in entries {
        let expired = now
            .duration_since(entry.modified)
            .is_ok_and(|age| age > max_age);
        if expired {
            remove.push(entry.path);
        } else {
            retained.push(entry);
        }
    }

    let mut retained_bytes = retained.iter().map(|entry| entry.bytes).sum::<u64>();
    let mut remove_count = 0usize;
    while retained.len().saturating_sub(remove_count) > max_files
        || retained_bytes > max_bytes
    {
        let Some(entry) = retained.get(remove_count) else {
            break;
        };
        retained_bytes = retained_bytes.saturating_sub(entry.bytes);
        remove.push(entry.path.clone());
        remove_count += 1;
    }
    remove
}

fn prune_default_wake_diagnostics(directory: &std::path::Path) -> Result<usize, String> {
    let mut entries = Vec::new();
    let read_dir = fs::read_dir(directory)
        .map_err(|err| format!("read {}: {err}", directory.display()))?;
    for item in read_dir {
        let Ok(item) = item else {
            continue;
        };
        let path = item.path();
        let Some(file_name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        let is_matching_wav = file_name.starts_with("wake-candidate-")
            && path
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case("wav"));
        if !is_matching_wav {
            continue;
        }
        let Ok(metadata) = item.metadata() else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        entries.push(WakeDiagnosticRetentionEntry {
            path,
            modified: metadata.modified().unwrap_or(std::time::UNIX_EPOCH),
            bytes: metadata.len(),
        });
    }

    let removals = wake_diagnostic_retention_plan(
        entries,
        std::time::SystemTime::now(),
        WAKE_DIAGNOSTIC_RETENTION_MAX_AGE,
        WAKE_DIAGNOSTIC_RETENTION_MAX_FILES,
        WAKE_DIAGNOSTIC_RETENTION_MAX_BYTES,
    );
    let mut removed = 0usize;
    for path in removals {
        match fs::remove_file(&path) {
            Ok(()) => removed += 1,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                log::warn!(
                    "[wake-phrase] diagnostic retention could not remove {}: {}",
                    path.display(),
                    err
                );
            }
        }
    }
    Ok(removed)
}

fn schedule_default_wake_diagnostic_cleanup(directory: std::path::PathBuf) {
    if WAKE_DIAGNOSTIC_CLEANUP_RUNNING
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }
    let spawn = std::thread::Builder::new()
        .name("wake-diag-retention".to_string())
        .spawn(move || {
            match prune_default_wake_diagnostics(&directory) {
                Ok(removed) if removed > 0 => log::info!(
                    "[wake-phrase] diagnostic rolling cleanup removed={} max_files={} max_bytes={} max_age_days=7",
                    removed,
                    WAKE_DIAGNOSTIC_RETENTION_MAX_FILES,
                    WAKE_DIAGNOSTIC_RETENTION_MAX_BYTES
                ),
                Ok(_) => {}
                Err(err) => log::warn!("[wake-phrase] diagnostic rolling cleanup failed: {err}"),
            }
            WAKE_DIAGNOSTIC_CLEANUP_RUNNING.store(false, Ordering::SeqCst);
        });
    if let Err(err) = spawn {
        WAKE_DIAGNOSTIC_CLEANUP_RUNNING.store(false, Ordering::SeqCst);
        log::warn!("[wake-phrase] diagnostic cleanup thread unavailable: {err}");
    }
}

fn save_bounded_wake_diagnostic(embedded_session_id: u32, outcome: &'static str, pcm: &[u8]) {
    // Prefer explicit env; otherwise always keep a small rolling ring under LocalAppData
    // so owner wake misses can be inspected without re-running with special flags.
    let explicit_directory = std::env::var(WAKE_DIAGNOSTIC_DIR_ENV).ok();
    let is_default_directory = explicit_directory.is_none();
    let directory = explicit_directory.unwrap_or_else(|| {
        let base = std::env::var("LOCALAPPDATA")
            .or_else(|_| std::env::var("APPDATA"))
            .unwrap_or_else(|_| ".".to_string());
        std::path::Path::new(&base)
            .join("Listener Type")
            .join("Logs")
            .join("wake-diag-live")
            .to_string_lossy()
            .into_owned()
    });
    if directory.trim().is_empty() {
        return;
    }
    let Ok(index) =
        WAKE_DIAGNOSTIC_CAPTURE_COUNT.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |current| {
            (current < WAKE_DIAGNOSTIC_MAX_CANDIDATES).then_some(current + 1)
        })
    else {
        return;
    };
    let directory = std::path::PathBuf::from(directory);
    if let Err(err) = fs::create_dir_all(&directory) {
        log::warn!("[wake-phrase] diagnostic directory unavailable: {err}");
        return;
    }
    let pcm_len = pcm.len().min(WAKE_DIAGNOSTIC_MAX_PCM_BYTES) & !1usize;
    let pcm = &pcm[..pcm_len];
    let mut wav = Vec::with_capacity(44 + pcm.len());
    let data_size = pcm.len() as u32;
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36u32.saturating_add(data_size)).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&16_000u32.to_le_bytes());
    wav.extend_from_slice(&32_000u32.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_size.to_le_bytes());
    wav.extend_from_slice(pcm);
    let timestamp_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let path = directory.join(format!(
        "wake-candidate-{timestamp_ms}-{}-{index:03}-session-{embedded_session_id}-{outcome}.wav",
        std::process::id()
    ));
    match fs::write(&path, wav) {
        Ok(()) => {
            log::info!(
                "[wake-phrase] bounded local diagnostic saved index={} embedded_session_id={} outcome={} pcm_ms={}",
                index,
                embedded_session_id,
                outcome,
                pcm.len() / 32
            );
            if is_default_directory {
                schedule_default_wake_diagnostic_cleanup(directory);
            }
        }
        Err(err) => log::warn!("[wake-phrase] diagnostic WAV write failed: {err}"),
    }
}

fn mark_hidden_automatic_candidate_active() {
    // Device key may have already asked to take over before ACTIVE was set.
    if DEVICE_KEY_DICTATION_TAKEOVER_PENDING.swap(false, Ordering::SeqCst) {
        HIDDEN_AUTOMATIC_CANDIDATE_STATE.store(
            HIDDEN_AUTOMATIC_CANDIDATE_PROMOTION_REQUESTED,
            Ordering::SeqCst,
        );
        log::info!(
            "[speaker-verification] device-key takeover pending applied as promotion on hidden candidate start"
        );
        return;
    }
    HIDDEN_AUTOMATIC_CANDIDATE_STATE.store(HIDDEN_AUTOMATIC_CANDIDATE_ACTIVE, Ordering::SeqCst);
}

fn clear_hidden_automatic_candidate() {
    HIDDEN_AUTOMATIC_CANDIDATE_STATE.store(HIDDEN_AUTOMATIC_CANDIDATE_NONE, Ordering::SeqCst);
    DEVICE_KEY_DICTATION_TAKEOVER_PENDING.store(false, Ordering::SeqCst);
}

fn clear_device_key_dictation_takeover_pending() {
    DEVICE_KEY_DICTATION_TAKEOVER_PENDING.store(false, Ordering::SeqCst);
}

pub(super) fn hidden_automatic_candidate_active() -> bool {
    HIDDEN_AUTOMATIC_CANDIDATE_STATE.load(Ordering::SeqCst) == HIDDEN_AUTOMATIC_CANDIDATE_ACTIVE
}

/// Device-key Dictation Start: prefer promote/ACTIVATE over TOGGLE-stop of a live
/// hidden automatic session. Returns true when the host should send VREC:ACTIVATE.
pub(super) fn note_device_key_dictation_start_intent() -> bool {
    DEVICE_KEY_DICTATION_TAKEOVER_PENDING.store(true, Ordering::SeqCst);
    if request_hidden_automatic_candidate_promotion() {
        log::info!(
            "[speaker-verification] device-key start promotes active hidden automatic candidate"
        );
        return true;
    }
    let state = HIDDEN_AUTOMATIC_CANDIDATE_STATE.load(Ordering::SeqCst);
    if state == HIDDEN_AUTOMATIC_CANDIDATE_PROMOTION_REQUESTED {
        log::info!(
            "[speaker-verification] device-key start reuses already-requested hidden promotion"
        );
        return true;
    }
    // Candidate not host-visible yet (KWS init / notify lag). Keep takeover pending;
    // firmware also maps TOGGLE→activate for hidden automatic sessions.
    log::info!(
        "[speaker-verification] device-key start takeover pending until hidden candidate is ready"
    );
    false
}

pub(super) fn request_hidden_automatic_candidate_promotion() -> bool {
    HIDDEN_AUTOMATIC_CANDIDATE_STATE
        .compare_exchange(
            HIDDEN_AUTOMATIC_CANDIDATE_ACTIVE,
            HIDDEN_AUTOMATIC_CANDIDATE_PROMOTION_REQUESTED,
            Ordering::SeqCst,
            Ordering::SeqCst,
        )
        .is_ok()
}

fn take_hidden_automatic_candidate_promotion() -> bool {
    HIDDEN_AUTOMATIC_CANDIDATE_STATE
        .compare_exchange(
            HIDDEN_AUTOMATIC_CANDIDATE_PROMOTION_REQUESTED,
            HIDDEN_AUTOMATIC_CANDIDATE_NONE,
            Ordering::SeqCst,
            Ordering::SeqCst,
        )
        .is_ok()
}

fn discard_pre_press_candidate_pcm(pcm: &mut Vec<u8>) -> usize {
    let discarded_pcm_bytes = pcm.len();
    pcm.clear();
    discarded_pcm_bytes
}

fn buffered_speaker_candidate_kind(
    start_origin: crate::embedded_audio::SessionStartOrigin,
    enrollment_armed: bool,
    enrolled: bool,
) -> Option<BufferedSpeakerCandidateKind> {
    if enrollment_armed {
        return Some(BufferedSpeakerCandidateKind::Enrollment);
    }
    match start_origin {
        crate::embedded_audio::SessionStartOrigin::User => None,
        // Automatic wake always enters the Verification gate path.
        // `speaker_verification::verify` already open-gates when no voiceprint is
        // enrolled (phrase hit alone accepts). Rejecting here when !enrolled made
        // "delete voiceprint" permanently disable wake — opposite of product intent.
        crate::embedded_audio::SessionStartOrigin::VoiceActivation => {
            let _ = enrolled;
            Some(BufferedSpeakerCandidateKind::Verification)
        }
        crate::embedded_audio::SessionStartOrigin::Unknown(_) => {
            Some(BufferedSpeakerCandidateKind::Rejected)
        }
    }
}

struct BufferedSpeakerCandidate {
    kind: BufferedSpeakerCandidateKind,
    pcm: Vec<u8>,
    wake_detector: Option<crate::wake_phrase::StreamingDetector>,
    /// Non-blocking detector init: begin buffers PCM immediately while this runs
    /// (~0.5–1s). Awaiting StreamingDetector::new before buffering made the
    /// capsule wait an extra second after the user already said 开始录音.
    wake_detector_init: Option<
        tauri::async_runtime::JoinHandle<Result<crate::wake_phrase::StreamingDetector, String>>,
    >,
    pending_phrase_match: Option<PendingAutomaticPhraseMatch>,
    #[cfg(target_os = "windows")]
    local_confirmation_task:
        Option<tauri::async_runtime::JoinHandle<Result<LocalWakeConfirmation, String>>>,
    local_confirmation_attempts: usize,
    local_confirmation_last_snapshot_bytes: usize,
    /// First KWS hit schedules an immediate local confirm instead of waiting for
    /// the next 5s/8s ladder rung (owner saw ~5–7s wake delay before accept).
    kws_prompted_local_confirm: bool,
    /// Local ASR returned Absent while KWS still hot (telemetry / retry pacing).
    kws_local_absent_count: u8,
    /// All completed midstream local Absent results. Terminal handling uses
    /// repeated evidence to skip an expensive ambient-only offline cascade.
    local_absent_count: u8,
    /// Wall clock of first live KWS hit — drives secondary-confirm budget
    /// (XiaoAi-style stage-2 timeout fail-open).
    kws_first_hit_at: Option<Instant>,
    /// Candidate PCM length (ms) at first live KWS hit — gates when an Absent
    /// may count toward hard-reject (phrase must have had time to finish).
    kws_first_hit_pcm_ms: Option<usize>,
    /// Recording capsule shown at first KWS hit (before local ExactStart) so the
    /// user is not left waiting with no UI while post-wake speech is already buffered.
    early_capsule_session_id: Option<SessionId>,
    kws_fed_bytes: usize,
    kws_total_ms: u64,
    started_at: Instant,
}

struct PendingAutomaticPhraseMatch {
    wake_match: crate::wake_phrase::Match,
    phrase_signal: denzic_voice_activation_v1_core::PhraseSignal,
    local_confirmation_ms: u64,
    owner_verification_start_ms: usize,
}

#[cfg(target_os = "windows")]
#[derive(Debug)]
struct LocalWakeConfirmation {
    matched: bool,
    phrase_relation: crate::wake_phrase::LocalPhraseRelation,
    transcript_chars: usize,
    inference_ms: u64,
    snapshot_pcm_ms: usize,
    recovered_keyword_end_seconds: Option<f32>,
}

const MAX_BUFFERED_SPEAKER_CANDIDATE_BYTES: usize = 2_100_000;
const STREAMING_KWS_FEED_BATCH_BYTES: usize = 1_600;
/// Proactive trailing-silence stop (改A) — DISABLED. A fixed energy-silence
/// threshold cannot distinguish a mid-sentence pause from a real
/// end-of-utterance, so any value that beats the firmware `auto_stop_silence`
/// timeout (observed 3-14s) truncates speech. User acceptance 2026-07-26:
/// "话还没说完就结束了". Set well above the firmware's max auto_stop so the
/// device's own endpointer always wins and this path stays dormant. The proper
/// fix is a content-aware endpoint (ASR sentence boundary) and/or device-side
/// wake word detection — see the 治本 plan. Do not lower this again until one of
/// those gates the dispatch.
const EMBEDDED_STREAMING_PROACTIVE_STOP_SILENCE_MS: u64 = 30_000;
const OWNER_VERIFICATION_START_MS: usize = 1_100;
const OWNER_VERIFICATION_START_BYTES: usize = OWNER_VERIFICATION_START_MS * 32;
const OWNER_VERIFICATION_SNAPSHOT_MS: [usize; 3] = [OWNER_VERIFICATION_START_MS, 1_800, 2_400];
// The generic protocol default is 1.8 s. Listener's four-character Mandarin
// phrase is already complete around 0.8-1.0 s in real captures, so start the
// local second chance here instead of making a streaming-KWS miss feel broken.
const LOCAL_CONFIRMATION_START_MS: usize = 1_000;
const LOCAL_CONFIRMATION_START_BYTES: usize = LOCAL_CONFIRMATION_START_MS * 32;
const LOCAL_CONFIRMATION_SNAPSHOT_MS: [usize; 6] = [
    LOCAL_CONFIRMATION_START_MS,
    1_400,
    1_800,
    2_400,
    3_000,
    5_000,
];
/// Once KWS already heard the phrase, do not wait for the 1.8s ladder floor.
/// ~0.8s covers a full "开始录音" plus a small pad.
const KWS_IMMEDIATE_LOCAL_CONFIRM_MIN_MS: usize = 800;
const KWS_IMMEDIATE_LOCAL_CONFIRM_MIN_BYTES: usize = KWS_IMMEDIATE_LOCAL_CONFIRM_MIN_MS * 32;
/// After a failed immediate confirm, re-try every 400ms of new audio while KWS
/// stays hot — avoids sitting on the 2.4/3.0/5.0s ladder rungs.
const KWS_LOCAL_CONFIRM_RETRY_MS: usize = 400;
const KWS_LOCAL_CONFIRM_RETRY_BYTES: usize = KWS_LOCAL_CONFIRM_RETRY_MS * 32;
/// Streaming KWS already supplies an absolute phrase boundary. Stage-2 only
/// needs nearby speech for precision; sending a long ambient prefix makes
/// Paraformer latency scale with unrelated audio. Cap remains 5_000 ms
/// (LST-WAKE-009); a shorter focus tail is tried on Absent before counting
/// hard-reject evidence.
const KWS_LOCAL_CONFIRM_MAX_PCM_MS: usize = 5_000;
const KWS_LOCAL_CONFIRM_MAX_PCM_BYTES: usize = KWS_LOCAL_CONFIRM_MAX_PCM_MS * 32;
/// Phrase-focused retry after a 5 s tail Absent. Real quiet miss session 288
/// ends the wake near 3.1 s inside a ~3.9 s candidate; a 1.6 s tail isolates
/// the phrase better than ambient-polluted full-candidate Paraformer ASR.
const KWS_LOCAL_CONFIRM_FOCUS_PCM_MS: usize = 1_600;
const KWS_LOCAL_CONFIRM_FOCUS_PCM_BYTES: usize = KWS_LOCAL_CONFIRM_FOCUS_PCM_MS * 32;
/// Do not spend the two-Absent hard-reject budget until the candidate has
/// continued ~one full Mandarin wake phrase after the first KWS hit. Session
/// 288 evidence: streaming KWS at 1.92 s with offline full-phrase end at
/// 3.1 s — two early Absents killed the candidate before the phrase finished.
const KWS_ABSENT_COUNT_MIN_POST_HIT_MS: usize = 1_000;
/// XiaoAi-style cascade after sensitive KWS hit:
///   stage-1 KWS (high recall) → stage-2 local wake verifier (precision)
/// Wait only inside the accepted phrase-tail budget for stage-2; then fail-open
/// as KeywordModel so an intermittently slow helper never makes wake feel
/// unresponsive. Explicit Absent still rejects. The remaining ~100 ms covers
/// actor polling plus recording-control/capsule dispatch under the 350 ms target.
const KWS_SECONDARY_CONFIRM_BUDGET_MS: u64 = 250;
/// Explicit local Absent count before midstream hard-reject (blocks short
/// prefix false wakes like "开始啥的"; one retry for noisy short clips).
const KWS_SECONDARY_ABSENT_REJECT_COUNT: u8 = 2;
/// Do not serialize the BLE actor behind multi-second auxiliary recall after
/// repeated local evidence already rejected an ambient candidate.
const TERMINAL_OFFLINE_SKIP_ABSENT_COUNT: u8 = 2;
const MIN_TERMINAL_OFFLINE_PCM_BYTES: usize = 16_000 * 2 * 2;
const TERMINAL_OFFLINE_RECALL_BUDGET_MS: u64 = 500;
const WAKE_END_PAD_SECONDS: f32 = 0.12;
const LOCAL_ONLY_START_ENDPOINT_MAX_SECONDS: f32 = 1.20;

fn owner_verification_window_ready(pcm_bytes: usize, enrolled: bool) -> bool {
    // No enrolled voiceprint → phrase hit alone is enough; do not stall for the
    // 1.1s owner speech window (that delay only exists for embedding quality).
    if !enrolled {
        return true;
    }
    pcm_bytes >= OWNER_VERIFICATION_START_BYTES
}

fn next_owner_verification_retry_ms(pcm_ms: usize) -> Option<usize> {
    OWNER_VERIFICATION_SNAPSHOT_MS
        .iter()
        .copied()
        .find(|snapshot_ms| *snapshot_ms > pcm_ms)
}

fn next_local_confirmation_snapshot_bytes(attempts: usize) -> Option<usize> {
    denzic_voice_activation_v1_core::confirmation_snapshot_ms(
        attempts,
        &LOCAL_CONFIRMATION_SNAPSHOT_MS,
    )
    .map(|milliseconds| milliseconds * 32)
}

fn local_confirmation_pcm(pcm: &[u8], has_keyword_model_hit: bool) -> Vec<u8> {
    tail_pcm_window(pcm, if has_keyword_model_hit {
        KWS_LOCAL_CONFIRM_MAX_PCM_BYTES
    } else {
        pcm.len()
    })
}

fn tail_pcm_window(pcm: &[u8], max_bytes: usize) -> Vec<u8> {
    if max_bytes == 0 || pcm.len() <= max_bytes {
        return pcm.to_vec();
    }
    let start = pcm.len() - max_bytes;
    let aligned_start = start + start % 2;
    pcm[aligned_start..].to_vec()
}

fn kws_phrase_focus_pcm(pcm: &[u8]) -> Vec<u8> {
    tail_pcm_window(pcm, KWS_LOCAL_CONFIRM_FOCUS_PCM_BYTES)
}

/// An explicit KWS-path Absent only hard-rejects after the candidate has grown
/// by about one full phrase past the first hit. Earlier Absents still retry.
fn kws_absent_counts_toward_reject(
    first_hit_pcm_ms: Option<usize>,
    snapshot_pcm_ms: usize,
) -> bool {
    match first_hit_pcm_ms {
        None => true,
        Some(hit_ms) => snapshot_pcm_ms >= hit_ms.saturating_add(KWS_ABSENT_COUNT_MIN_POST_HIT_MS),
    }
}

fn should_run_terminal_offline_recall(pcm_bytes: usize, local_absent_count: u8) -> bool {
    pcm_bytes >= MIN_TERMINAL_OFFLINE_PCM_BYTES
        && local_absent_count < TERMINAL_OFFLINE_SKIP_ABSENT_COUNT
}

/// Bytes of candidate PCM to discard before ASR for an automatic wake accept.
/// Uses KWS/local end time when available; LocalTranscript must not force 0 —
/// that shipped pre-wake speech ("好贵啊…开始录音，帮我看…") into the capsule.
fn post_wake_pcm_offset_bytes(wake_end_seconds: f32, pcm_len: usize) -> usize {
    denzic_voice_activation_v1_core::pcm_offset_after_activation(
        wake_end_seconds,
        WAKE_END_PAD_SECONDS,
        32_000,
        pcm_len,
        2,
    )
}

fn wake_phrase_tail_to_capsule_ms(wake_end_seconds: f32, capsule_request_ms: u64) -> u64 {
    if !wake_end_seconds.is_finite() || wake_end_seconds <= 0.0 {
        return capsule_request_ms;
    }
    let wake_end_ms = (wake_end_seconds * 1_000.0).round().max(0.0) as u64;
    capsule_request_ms.saturating_sub(wake_end_ms)
}

#[cfg(target_os = "windows")]
fn refined_wake_end_seconds(
    keyword_end_seconds: f32,
    confirmation: &LocalWakeConfirmation,
    phrase_chars: usize,
) -> f32 {
    denzic_voice_activation_v1_core::refined_local_wake_end_seconds(
        denzic_voice_activation_v1_core::LocalConfirmationBoundaryInput {
            keyword_end_seconds,
            recovered_keyword_end_seconds: confirmation.recovered_keyword_end_seconds,
            phrase_relation: confirmation.phrase_relation,
            transcript_chars: confirmation.transcript_chars,
            phrase_chars,
            snapshot_pcm_ms: confirmation.snapshot_pcm_ms,
            end_pad_seconds: WAKE_END_PAD_SECONDS,
            local_endpoint_max_seconds: LOCAL_ONLY_START_ENDPOINT_MAX_SECONDS,
        },
    )
}

#[cfg(target_os = "windows")]
fn local_confirmation_can_activate(
    has_keyword_model_hit: bool,
    relation: crate::wake_phrase::LocalPhraseRelation,
) -> bool {
    denzic_voice_activation_v1_core::local_confirmation_can_activate(
        has_keyword_model_hit,
        relation,
    )
}

#[cfg(target_os = "windows")]
fn secondary_fallback_can_accept_keyword(
    keyword_model_hit: bool,
    explicit_absent_count: u8,
) -> bool {
    matches!(
        denzic_voice_activation_v1_core::decide_secondary_fallback(
            denzic_voice_activation_v1_core::SecondaryFallbackInput {
                keyword_model_hit,
                explicit_absent_count,
                secondary_unavailable_or_timed_out: true,
            },
        ),
        denzic_voice_activation_v1_core::SecondaryFallbackDecision::AcceptKeywordModel
    )
}

#[cfg(target_os = "windows")]
fn completed_secondary_absent_is_authoritative(
    relation: crate::wake_phrase::LocalPhraseRelation,
    transcript_chars: usize,
    phrase_chars: usize,
) -> bool {
    matches!(
        denzic_voice_activation_v1_core::decide_completed_secondary(
            denzic_voice_activation_v1_core::CompletedSecondaryInput {
                relation,
                transcript_chars,
                phrase_chars,
            },
        ),
        denzic_voice_activation_v1_core::CompletedSecondaryDecision::RejectExplicitAbsent
    )
}

#[cfg(target_os = "windows")]
fn run_local_wake_confirmation_once(
    pcm: &[u8],
    phrase: &str,
) -> Result<LocalWakeConfirmation, String> {
    let snapshot_pcm_ms = pcm.len() / 32;
    // Firmware AFE owns AGC. The local helper receives the same waveform with
    // only an attenuation limiter for the -3 dBFS host ceiling.
    let limited = crate::wake_phrase::limit_pcm16_for_confirmation(pcm);
    let result = crate::asr::local::wake_helper::confirm(&limited, phrase, Duration::from_secs(4))
        .map_err(|err| format!("local wake confirmation failed: {err}"))?;
    Ok(LocalWakeConfirmation {
        matched: result.matched,
        phrase_relation: result.phrase_relation,
        transcript_chars: result.transcript_chars,
        inference_ms: result.inference_ms,
        snapshot_pcm_ms,
        recovered_keyword_end_seconds: None,
    })
}

#[cfg(target_os = "windows")]
fn spawn_local_wake_confirmation(
    _inner: &Arc<Inner>,
    pcm: Vec<u8>,
    phrase: String,
    recover_keyword_boundary: bool,
) -> tauri::async_runtime::JoinHandle<Result<LocalWakeConfirmation, String>> {
    tauri::async_runtime::spawn_blocking(move || {
        let started = Instant::now();
        // Exploratory (no KWS yet) keeps the provided snapshot as-is.
        // KWS path: primary is the LST-WAKE-009 5 s tail; on Absent, retry a
        // short phrase-focus tail before counting hard-reject evidence.
        let primary = if recover_keyword_boundary {
            pcm.clone()
        } else {
            local_confirmation_pcm(&pcm, true)
        };
        let mut result = run_local_wake_confirmation_once(&primary, &phrase)?;
        if !recover_keyword_boundary && !result.matched {
            let focus = kws_phrase_focus_pcm(&pcm);
            // Skip duplicate work when primary already was the short focus tail.
            if focus.len() < primary.len() {
                let focused = run_local_wake_confirmation_once(&focus, &phrase)?;
                if focused.matched {
                    result = focused;
                } else {
                    result.inference_ms = result.inference_ms.saturating_add(focused.inference_ms);
                }
            }
        }
        let recovered_keyword_end_seconds = if recover_keyword_boundary
            && result.matched
            && matches!(
                result.phrase_relation,
                crate::wake_phrase::LocalPhraseRelation::ExactStart
                    | crate::wake_phrase::LocalPhraseRelation::PhoneticStart
            )
            && result.transcript_chars <= phrase.chars().count()
        {
            crate::wake_phrase::detect(&pcm, &phrase)
                .map_err(|err| format!("recover local wake boundary: {err}"))?
                .map(|found| found.end_seconds)
        } else {
            None
        };
        result.recovered_keyword_end_seconds = recovered_keyword_end_seconds;
        result.inference_ms = result
            .inference_ms
            .max(started.elapsed().as_millis() as u64);
        Ok(result)
    })
}

struct EmbeddedStreamingDictation {
    collector: crate::embedded_audio::StreamingSessionCollector,
    session: Option<EmbeddedAudioDictationSession>,
    speaker_candidate: Option<BufferedSpeakerCandidate>,
    embedded_session_id: Option<u32>,
    transcript: Option<crate::embedded_audio::EmbeddedAudioTranscriptResult>,
    pending_stop_expected_packet_count: Option<u16>,
    /// When set, continuous background will force-finish a STOP that never
    /// recovered missing packets so capture can keep TYPE:READY open.
    pending_stop_force_after: Option<Instant>,
    terminal_received: bool,
    keep_listening_after_pipeline_errors: bool,
}

impl Default for EmbeddedStreamingDictation {
    fn default() -> Self {
        Self {
            collector: crate::embedded_audio::StreamingSessionCollector::default(),
            session: None,
            speaker_candidate: None,
            embedded_session_id: None,
            transcript: None,
            pending_stop_expected_packet_count: None,
            pending_stop_force_after: None,
            terminal_received: false,
            keep_listening_after_pipeline_errors: false,
        }
    }
}

/// 跑流式润色路径（opt-in，跨平台）。
///
/// 平台差异：
/// - **macOS**：`switch_to_ascii` 切到 ABC 输入源（规避 CJK / 日文 IME 拦截 Unicode 事件），
///   session 结束 `restore_input_source` 切回。`type_unicode_chunk` 走 CGEvent FFI。
/// - **Windows**：`switch_to_ascii` 是 no-op（SendInput Unicode 绕过 TSF）；
///   `type_unicode_chunk` 走 `SendInput(KEYEVENTF_UNICODE)`。
/// - **Linux（实验）**：`switch_to_ascii` 是 no-op；`type_unicode_chunk` 走 enigo
///   `Keyboard::text`。X11 / XTest 稳定，Wayland 看 compositor 给不给 libei 权限。
///
/// 通用流程：
/// 1. `switch_to_ascii`（macOS）/ no-op（其他）；失败则降级回一次性 `polish_or_passthrough`。
/// 2. 起一个 `spawn_blocking` 后台任务，从 mpsc 收 SSE delta，逐 delta 调
///    `type_unicode_chunk` 模拟键盘事件落到光标处。串行有序，无竞态。
/// 3. 调 `polish_or_passthrough_streaming`，`on_delta` 把 chunk 塞进 mpsc。
/// 4. 流结束 / 失败 / 取消 → drop mpsc 发送端 → typer 任务 drain 完剩余 delta 退出 →
///    `restore_input_source` 恢复用户原输入源（macOS 才有意义，其他平台 no-op）。
/// 5. 返回 `(polished, polish_error, already_streamed)`：
///    - 成功：`(text, None, true)` — 字符已经在屏幕上，调用方应当跳过 `inserter.insert`
///    - 失败：`(raw_text, Some(reason), false)` — 流式过程出错，调用方走 raw 一次性兜底
///    - 不支持：`run_streaming_polish` 内部直接调 `polish_or_passthrough` 透明降级
///
/// **不在流式路径里做**：`apply_chinese_script_preference` / `apply_correction_rules`
/// 这两步在 v1 跳过 —— 字符已经一边流一边落出去了，不好回退。需要的话只能关 toggle 走
/// 一次性路径。
#[allow(clippy::too_many_arguments)]
async fn run_streaming_polish(
    inner: &Arc<Inner>,
    raw: &RawTranscript,
    mode: PolishMode,
    hotwords: &[String],
    style_system_prompt: &str,
    working_languages: &[String],
    chinese_script_preference: crate::types::ChineseScriptPreference,
    output_language_preference: crate::types::OutputLanguagePreference,
    llm_thinking_enabled: bool,
    front_app: Option<&str>,
    prior_turns: &[(String, String)],
) -> (String, Option<String>, bool) {
    log::info!(
        "[coord] streaming_insert path ENTER (raw_chars={})",
        raw.text.chars().count()
    );

    let app = inner.app.lock().clone();
    let Some(app) = app else {
        log::warn!("[coord] streaming_insert: no AppHandle in Inner; fall back to one-shot");
        let (p, e) = polish_or_passthrough(
            raw,
            mode,
            hotwords,
            style_system_prompt,
            working_languages,
            chinese_script_preference,
            output_language_preference,
            llm_thinking_enabled,
            front_app,
            prior_turns,
        )
        .await;
        return (p, e, false);
    };

    // 1. 切到 ABC 输入源。失败则降级 —— 流式路径上 CJK IME 拦截不是可恢复错误。
    log::info!("[coord] streaming_insert: switching input source to ABC");
    let prev_ime = match crate::unicode_keystroke::switch_to_ascii(&app).await {
        Ok(prev) => {
            log::info!(
                "[coord] streaming_insert: switched to ABC (had_previous={})",
                prev.is_some()
            );
            prev
        }
        Err(e) => {
            log::warn!(
                "[coord] streaming_insert: switch_to_ascii failed: {e}; fall back to one-shot"
            );
            let (p, err) = polish_or_passthrough(
                raw,
                mode,
                hotwords,
                style_system_prompt,
                working_languages,
                chinese_script_preference,
                output_language_preference,
                llm_thinking_enabled,
                front_app,
                prior_turns,
            )
            .await;
            return (p, err, false);
        }
    };

    // 2. 起 typer 后台任务：从 mpsc 收 delta，串行调 type_unicode_chunk。
    // 同时累积 typed_text：屏幕上真正落字的内容，用于（a）SSE 中途失败时让 history
    // 与用户实际看到的内容一致；（b）pr-agent #412 反馈 \"saved output diverges
    // from what the user actually sees\"。
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let typer_handle = tokio::task::spawn_blocking(move || {
        let mut rx = rx;
        let mut typed_text = String::new();
        let mut first_failure: Option<String> = None;
        while let Some(delta) = rx.blocking_recv() {
            if first_failure.is_some() {
                // 一旦类型链路出错（如 Secure Input 启用），后续 delta 全部丢弃，但仍
                // 把 mpsc drain 完，避免发送端阻塞。
                continue;
            }
            let delta_chars = delta.chars().count();
            match crate::unicode_keystroke::type_unicode_chunk(&delta) {
                Ok(typed_chars) => {
                    let appended = append_typed_prefix(&mut typed_text, &delta, typed_chars);
                    if appended < delta_chars {
                        let reason = format!(
                            "type_unicode_chunk typed only {appended}/{delta_chars} chars without error"
                        );
                        log::error!(
                            "[coord] streaming_insert: {reason} at typed={} chars; \
                             dropping remaining deltas",
                            typed_text.chars().count()
                        );
                        first_failure = Some(reason);
                    }
                }
                Err(e) => {
                    append_typed_prefix(&mut typed_text, &delta, e.typed_chars());
                    log::error!(
                        "[coord] streaming_insert: type_unicode_chunk failed at typed={} chars: {e}; \
                         dropping remaining deltas",
                        typed_text.chars().count()
                    );
                    first_failure = Some(e.to_string());
                }
            }
        }
        (typed_text, first_failure)
    });

    // 3. 调流式润色，on_delta 塞 mpsc；should_cancel 检查 dictation 取消旗。
    let inner_for_cancel = Arc::clone(inner);
    let should_cancel = move || inner_for_cancel.state.lock().cancelled;
    let outcome = super::polish_or_passthrough_streaming(
        raw,
        mode,
        hotwords,
        style_system_prompt,
        working_languages,
        chinese_script_preference,
        output_language_preference,
        llm_thinking_enabled,
        front_app,
        prior_turns,
        move |delta: &str| {
            let _ = tx.send(delta.to_string());
        },
        should_cancel,
    )
    .await;
    // tx 已经被 move 进 on_delta 闭包；闭包随 polish_or_passthrough_streaming 返回
    // 而 drop，typer 那侧 blocking_recv 拿到 None 自然退出。

    // 4. 等 typer 把缓冲 drain 完，拿到实际落字的全文 + 第一条失败原因。
    let (typed_text, typer_failure) = typer_handle.await.unwrap_or_else(|e| {
        log::error!("[coord] streaming_insert: typer task join failed: {e}");
        (String::new(), Some(format!("typer join: {e}")))
    });
    let typed_chars = typed_text.chars().count();
    log::info!("[coord] streaming_insert: typer drained, typed {typed_chars} chars");

    // 5. 无论流是否成功，都恢复用户原输入源。
    log::info!("[coord] streaming_insert: restoring input source");
    if let Err(e) = crate::unicode_keystroke::restore_input_source(&app, prev_ime).await {
        log::warn!("[coord] streaming_insert: restore_input_source failed: {e}");
    } else {
        log::info!("[coord] streaming_insert: input source restored");
    }

    // 6. 把 outcome 翻译成 (polished, polish_error, already_streamed)。
    match outcome {
        super::StreamingPolishOutcome::Streamed(text) => {
            log::info!(
                "[coord] streaming_insert SUCCESS: polished_chars={} typed_chars={} typer_err={:?}",
                text.chars().count(),
                typed_chars,
                typer_failure
            );
            // 边界 case：polish 成功但 typer 在第一字就失败（最常见：session 开始时
            // 已处于 Secure Input；或 SendInput / enigo 拒绝）。屏幕上一字未见，
            // already_streamed=true 会让上层跳过 inserter，最终用户看不到任何内容。
            // 这里显式回退到一次性兜底，让正常 inserter 路径写出 polish 结果。
            // pr-agent #412 反馈 \"Missing fallback\"。
            if typed_chars == 0 {
                if let Some(reason) = typer_failure {
                    log::warn!(
                        "[coord] streaming_insert: zero chars typed despite polish success ({reason}); falling back to one-shot inserter"
                    );
                    return (text, Some(reason), false);
                }
            }
            // 先确定 final_text —— typer 中途失败时屏幕只有 typed_text 这一段，
            // history 记完整 polish 反而会让用户复盘困惑。让 history / clipboard /
            // 后续逻辑统统用 final_text，三处保持一致。
            // pr-agent #412 反馈 \"Clipboard Mismatch\"：之前先写 text 到剪贴板再
            // 决定 typer 是否中途失败，导致 Cmd+V 粘出用户屏幕上没见过的内容。
            let (final_text, polish_err) = match typer_failure {
                Some(e) => (typed_text, Some(format!("typing partially failed: {e}"))),
                None => (text, None),
            };
            (final_text, polish_err, true)
        }
        super::StreamingPolishOutcome::UnsupportedFallback => {
            log::info!(
                "[coord] streaming_insert: dispatch reported unsupported, fall back to one-shot"
            );
            let (p, e) = polish_or_passthrough(
                raw,
                mode,
                hotwords,
                style_system_prompt,
                working_languages,
                chinese_script_preference,
                output_language_preference,
                llm_thinking_enabled,
                front_app,
                prior_turns,
            )
            .await;
            (p, e, false)
        }
        super::StreamingPolishOutcome::Failed(reason) => {
            log::warn!(
                "[coord] streaming_insert FAILED: {reason}; typed {typed_chars} chars before failure"
            );
            // 流式失败但已经流了一部分 chars：用户屏幕上有半截 polish。history 应当
            // 跟屏幕一致 —— 记 typed_text 而不是 raw.text，否则保存内容跟用户看见的
            // 内容会分叉（pr-agent #412 \"Wrong final text\" 反馈）。
            // 一字都没流时 typed_text 是空串，回到 raw 一次性兜底。
            if typed_chars > 0 {
                (
                    typed_text,
                    Some(format!(
                        "streaming polish failed mid-stream after {typed_chars} chars: {reason}"
                    )),
                    true,
                )
            } else {
                (raw.text.clone(), Some(reason), false)
            }
        }
    }
}

fn finalize_polished_text(
    polished: String,
    translation_active: bool,
    _raw_uses_llm: bool,
    mode: PolishMode,
    polish_error: &Option<String>,
    chinese_script_preference: crate::types::ChineseScriptPreference,
    correction_rules: &[crate::types::CorrectionRule],
    already_streamed: bool,
) -> String {
    if already_streamed {
        return polished;
    }
    let should_force_script = if translation_active {
        polish_error.is_some()
    } else {
        mode == PolishMode::Raw || polish_error.is_some()
    };
    let polished = if should_force_script {
        apply_chinese_script_preference(&polished, chinese_script_preference)
    } else {
        polished
    };
    if correction_rules.is_empty() {
        polished
    } else {
        let corrected = apply_correction_rules(&polished, correction_rules);
        if corrected != polished {
            log::info!(
                "[coord] correction rules adjusted final text ({} → {} chars)",
                polished.chars().count(),
                corrected.chars().count()
            );
        }
        corrected
    }
}

fn streaming_insert_eligible(
    streaming_insert_enabled: bool,
    translation_active: bool,
    mode: PolishMode,
    raw_uses_llm: bool,
    wayland_session: bool,
) -> bool {
    streaming_insert_enabled
        && !translation_active
        && (mode != PolishMode::Raw || raw_uses_llm)
        && !wayland_session
}

fn wayland_done_message(status: InsertStatus, polish_failed: bool) -> Option<String> {
    match status {
        InsertStatus::Inserted | InsertStatus::PasteSent => None,
        InsertStatus::CopiedFallback => Some(if polish_failed {
            "Wayland 未启用自动输入，已复制原文到剪贴板，请手动粘贴".to_string()
        } else {
            "Wayland 未启用自动输入，已复制到剪贴板，请手动粘贴".to_string()
        }),
        InsertStatus::Failed => Some("Wayland 未启用自动输入，剪贴板写入失败".to_string()),
    }
}

fn default_done_message(status: InsertStatus, polish_failed: bool) -> Option<String> {
    if polish_failed {
        // polish 失败仍写 history error_code，但录音原文已成功落地时不要把整个
        // dictation 呈现成失败；否则无效 LLM key 会让成功录音看起来像回退。
        match status {
            InsertStatus::Inserted => None,
            InsertStatus::PasteSent => Some("已尝试粘贴原文".to_string()),
            InsertStatus::CopiedFallback => Some(if cfg!(target_os = "windows") {
                "已复制原文，请 Ctrl+V".to_string()
            } else {
                "已复制原文，请粘贴".to_string()
            }),
            InsertStatus::Failed => Some("润色不可用，插入失败".to_string()),
        }
    } else {
        match status {
            InsertStatus::Inserted => None,
            InsertStatus::PasteSent => Some("已尝试粘贴".to_string()),
            InsertStatus::CopiedFallback => Some(if cfg!(target_os = "windows") {
                "已复制，请 Ctrl+V".to_string()
            } else {
                "已复制，请粘贴".to_string()
            }),
            InsertStatus::Failed => Some("插入失败".to_string()),
        }
    }
}

fn device_processing_final_succeeded(status: InsertStatus, error_code: Option<&str>) -> bool {
    if status == InsertStatus::Failed {
        return false;
    }
    matches!(error_code, None | Some("polishFailed"))
}
