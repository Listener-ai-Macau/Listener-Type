// Explicit file transcription, isolated from live dictation and insertion.
use std::collections::VecDeque;
use crate::asr::VolcengineStreamingASR;
use crate::asr::AudioConsumer as AsrAudioConsumer;

const AUDIO_FILE_MAX_PCM: usize = 60 * 32_000;
const AUDIO_FILE_MAX_WAV: usize = AUDIO_FILE_MAX_PCM + 65_536;

#[derive(Default)]
struct AudioFileRequests {
    active: Option<(String, tokio::sync::watch::Sender<bool>)>,
    cancelled: VecDeque<String>,
}
static AUDIO_FILE_REQUESTS: OnceLock<Mutex<AudioFileRequests>> = OnceLock::new();

fn audio_file_requests() -> &'static Mutex<AudioFileRequests> {
    AUDIO_FILE_REQUESTS.get_or_init(|| Mutex::new(AudioFileRequests::default()))
}

fn audio_file_pcm(wav: &[u8]) -> Result<Vec<u8>, String> {
    if wav.len() > AUDIO_FILE_MAX_WAV { return Err("audio_file_too_long".into()); }
    let pcm = crate::embedded_audio::read_wav_pcm16le(wav)
        .map_err(|_| "audio_file_format".to_owned())?;
    if pcm.is_empty() || pcm.len() % 2 != 0 { return Err("audio_file_format".into()); }
    if pcm.len() > AUDIO_FILE_MAX_PCM { return Err("audio_file_too_long".into()); }
    Ok(pcm)
}

impl AudioFileRequests {
    fn begin(&mut self, id: &str) -> Result<tokio::sync::watch::Receiver<bool>, String> {
        if let Some(index) = self.cancelled.iter().position(|old| old == id) {
            self.cancelled.remove(index);
            return Err("audio_file_cancelled".into());
        }
        if self.active.is_some() { return Err("audio_file_busy".into()); }
        let (tx, rx) = tokio::sync::watch::channel(false);
        self.active = Some((id.to_owned(), tx));
        Ok(rx)
    }
    fn cancel(&mut self, id: String) {
        if let Some((active, tx)) = &self.active {
            if active == &id { let _ = tx.send(true); return; }
        }
        if !self.cancelled.contains(&id) { self.cancelled.push_back(id); }
        while self.cancelled.len() > 16 { self.cancelled.pop_front(); }
    }
    fn finish(&mut self, id: &str) {
        if self.active.as_ref().is_some_and(|(active, _)| active == id) { self.active = None; }
    }
}

struct AudioFileLease { id: String, asr: Arc<VolcengineStreamingASR> }
impl Drop for AudioFileLease {
    fn drop(&mut self) {
        self.asr.cancel();
        audio_file_requests().lock().finish(&self.id);
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioFileTranscript { text: String, audio_duration_ms: u64, elapsed_ms: u64 }

#[tauri::command]
pub fn cancel_audio_file_trial(request_id: String) -> Result<(), String> {
    if uuid::Uuid::parse_str(&request_id).is_err() { return Err("audio_file_invalid_request".into()); }
    audio_file_requests().lock().cancel(request_id);
    Ok(())
}

#[tauri::command]
pub async fn transcribe_audio_file_trial(
    coord: CoordinatorState<'_>, request_id: String, wav_bytes: Vec<u8>,
) -> Result<AudioFileTranscript, String> {
    if uuid::Uuid::parse_str(&request_id).is_err() { return Err("audio_file_invalid_request".into()); }
    // Validate the bounded user-selected payload before opening any service.
    let pcm = audio_file_pcm(&wav_bytes)?;
    let prefs = coord.prefs().get();
    if prefs.active_asr_provider != "volcengine"
        || prefs.cloud_model_route == crate::asr::cloud_gateway::CloudModelRoute::Managed {
        return Err("audio_file_provider".into());
    }
    if coord.dictation_phase_for_cli() != crate::coordinator_state::SessionPhase::Idle {
        return Err("audio_file_recording".into());
    }
    let credentials = crate::coordinator::read_volc_credentials();
    if credentials.app_id.trim().is_empty() || credentials.access_token.trim().is_empty()
        || credentials.resource_id.trim().is_empty() { return Err("audio_file_setup".into()); }
    let proxy = crate::coordinator::read_asr_proxy_config("volcengine")
        .map_err(|_| "audio_file_proxy".to_owned())?;
    // A separate recognizer: no capture, owner model, global style, target,
    // History, clipboard, device command or live coordinator session mutation.
    let asr = Arc::new(VolcengineStreamingASR::new_with_proxy_config(credentials, vec![], proxy));
    let mut cancelled = audio_file_requests().lock().begin(&request_id)?;
    let _lease = AudioFileLease { id: request_id, asr: Arc::clone(&asr) };
    let started = Instant::now();
    let duration = pcm.len() as u64 / 32;
    let recognize = async {
        asr.open_session().await.map_err(|e| e.to_string())?;
        // This path owns all input already; unlike live dictation there is no
        // deferred bridge to publish delivery readiness. The provider must
        // leave Opening before the final frame is allowed to drain/send.
        asr.mark_audio_delivery_ready();
        for chunk in pcm.chunks(6_400) { asr.consume_pcm_chunk(chunk); }
        asr.send_last_frame().await.map_err(|e| e.to_string())?;
        let result = asr.await_final_result().await.map_err(|e| e.to_string())?;
        if result.text.trim().is_empty() { return Err("audio_file_empty".into()); }
        Ok(AudioFileTranscript { text: result.text, audio_duration_ms: duration,
            elapsed_ms: started.elapsed().as_millis() as u64 })
    };
    tokio::select! {
        biased;
        _ = cancelled.changed() => Err("audio_file_cancelled".into()),
        result = tokio::time::timeout(Duration::from_secs(45), recognize) =>
            result.map_err(|_| "audio_file_timeout".to_owned())?,
    }
}

#[cfg(test)]
mod audio_file_trial_tests {
    use super::*;
    #[test]
    fn selected_wav_is_bounded_and_never_silently_truncated() {
        let valid = crate::asr::wav::encode_wav_16k_mono(&[9, -8]);
        assert_eq!(audio_file_pcm(&valid).unwrap(), vec![9, 0, 248, 255]);
        let mut stereo = valid.clone(); stereo[22] = 2;
        assert_eq!(audio_file_pcm(&stereo).unwrap_err(), "audio_file_format");
        let mut odd = valid.clone(); odd.pop();
        assert_eq!(audio_file_pcm(&odd).unwrap_err(), "audio_file_format");
        let too_long = crate::asr::wav::encode_wav_16k_mono(&vec![1; 960_001]);
        assert_eq!(audio_file_pcm(&too_long).unwrap_err(), "audio_file_too_long");
    }
    #[test]
    fn cancellation_before_dispatch_is_not_lost_and_other_requests_are_not_cancelled() {
        let mut r = AudioFileRequests::default();
        r.cancel("cancelled-before-start".into());
        assert!(r.begin("cancelled-before-start").is_err());
        let rx = r.begin("one").unwrap();
        assert!(r.begin("two").is_err());
        r.cancel("two".into()); assert!(!*rx.borrow());
        r.finish("two"); assert!(r.active.is_some());
        r.cancel("one".into()); assert!(*rx.borrow());
        r.finish("one"); assert!(r.active.is_none());
        assert!(r.begin("two").is_err());
        assert!(r.begin("three").is_ok());
    }
}
