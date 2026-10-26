//! Both question input sources share the same bounded recognition closeout.
//! Preserve the captured audio for one existing-provider recovery; never ask
//! an answer model to guess a question whose recognition connection failed.
use super::*;

pub(super) async fn finish_question_speech<F>(
    asr: &Arc<VolcengineStreamingASR>, should_cancel: F,
    on_recovery: impl Fn(),
) -> Result<RawTranscript, String>
where F: Fn() -> bool,
{
    use crate::asr::volcengine::VolcengineASRError;
    if should_cancel() { asr.cancel(); return Err("qa_cancelled".into()); }
    let primary = match asr.send_last_frame().await {
        Ok(()) => match tokio::time::timeout(
            Duration::from_secs(COORDINATOR_GLOBAL_TIMEOUT_SECS), asr.await_final_result()).await {
            Ok(result) => result,
            Err(_) => Err(VolcengineASRError::FinalResultTimeout),
        },
        Err(error) => Err(error),
    };
    if should_cancel() { asr.cancel(); return Err("qa_cancelled".into()); }
    match primary {
        Ok(raw) => Ok(raw),
        Err(error) if error.permits_full_audio_replay() => {
            // The ASR object already enforces a single retained-audio claim,
            // uses the same credentials/options and owns a bounded retry budget.
            // The question has not been published or inserted, so recovery
            // cannot duplicate a question/answer or editor delivery.
            log::warn!("[qa] primary speech closeout failed; recovering retained audio once: {error}");
            on_recovery();
            let budget = asr.full_audio_replay_timeout();
            asr.cancel();
            let result = tokio::time::timeout(budget, asr.replay_retained_audio_once()).await;
            if should_cancel() { return Err("qa_cancelled".into()); }
            match result {
                Ok(Ok(raw)) => Ok(raw),
                Ok(Err(error)) => {
                    log::warn!("[qa] retained speech recovery failed: {error}");
                    Err("qa_speech_recovery_failed".into())
                }
                Err(_) => {
                    log::warn!("[qa] retained speech recovery exceeded its bounded budget");
                    Err("qa_speech_recovery_failed".into())
                }
            }
        }
        Err(error) => { asr.cancel(); Err(error.to_string()) }
    }
}

pub(super) fn show_recovery(inner: &Arc<Inner>, owner: SessionId) {
    let qa = inner.qa_state.lock();
    if qa.session_id != owner || qa.cancelled || qa.phase != QaPhase::Processing { return; }
    if let Some(app) = inner.app.lock().clone() {
        let _ = app.emit_to("qa", "qa:state", serde_json::json!({
            "kind":"loading", "recovering_speech":true,
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    #[ignore = "Requires explicit private-profile existing-service QA acceptance opt-in"]
    async fn writer_timeout_recovers_saved_question_without_changing_credentials() {
        assert_eq!(std::env::var("LISTENER_TYPE_QA_EXISTING_SERVICES_OPT_IN").as_deref(), Ok("1"));
        assert!(std::env::var("LISTENER_TYPE_CREDENTIAL_NAMESPACE").is_ok());
        let root = std::path::PathBuf::from(std::env::var("LISTENER_TYPE_DATA_DIR").unwrap());
        assert!(root.to_string_lossy().contains("accept131-rust-"));
        let pcm = std::fs::read(std::env::var("LISTENER_TYPE_QA_FIXED_PCM").unwrap()).unwrap();
        assert!((32_000..=256_000).contains(&pcm.len()) && pcm.len() % 2 == 0);
        let asr = Arc::new(VolcengineStreamingASR::new(read_volc_credentials(), Vec::new()));
        crate::asr::AudioConsumer::consume_pcm_chunk(asr.as_ref(), &pcm);
        assert_eq!(asr.retained_pcm_snapshot(), pcm);
        // Same failure category and exact diagnostic as the real193 receipt.
        // No broken socket or hardware is created; only recovery uses the
        // already-authorized provider with this non-private generated speech.
        asr.mark_audio_delivery_failed(crate::asr::volcengine::VolcengineASRError::ConnectionFailed(
            "websocket send timed out after 2500 ms".into()));
        let recoveries = std::sync::atomic::AtomicUsize::new(0);
        let raw = finish_question_speech(&asr, || false,
            || { recoveries.fetch_add(1, Ordering::SeqCst); }).await.unwrap();
        assert_eq!(recoveries.load(Ordering::SeqCst), 1);
        let key = raw.text.replace([' ', '，', '？', '?'], "");
        assert!(key.contains("一加一") || key.contains("1+1"), "{}", raw.text);
        std::fs::write(root.join("qa-existing-services-result.json"), serde_json::to_vec_pretty(&serde_json::json!({
            "passed":true,"scope":"Exact real193 writer-timeout fault category injected into retained synthetic Question PCM; production shared finalizer makes one actual existing-provider ASR replay. No real network fault, KWS/owner/microphone/RF/native UI or answer-model qualification.",
            "injectedError":"websocket send timed out after 2500 ms","recoveryCallbacks":recoveries.load(Ordering::SeqCst),
            "recognizedQuestion":raw.text,"pcmBytes":pcm.len(),"newProviderOrCredentials":false,
        })).unwrap()).unwrap();
    }
}
