//! Explicit synthetic-audio check. Never reads a microphone, editor or clipboard.
use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "Requires explicitly staged OS-vault API key and a synthetic WAV; calls the real speech provider"]
async fn speech_api_key_live_synthetic_recognition() {
    assert_eq!(std::env::var("LISTENER_API_KEY_LIVE_PROBE").as_deref(), Ok("1"));
    let file = std::env::var_os("LISTENER_API_KEY_SYNTHETIC_WAV").expect("synthetic input path required");
    let pcm = denzic_audio_v1_core::read_wav_pcm16le(&std::fs::read(file).unwrap()).unwrap();
    assert!(!pcm.is_empty());
    let key = keyring::Entry::new("com.listener.type", "volcengine.pending_api_key.20261114")
        .unwrap().get_password().expect("explicit staged key required");
    let credentials = VolcengineCredentials { api_key: key, app_id: String::new(), access_token: String::new(), resource_id: "volc.bigasr.sauc.duration".into() };
    let asr = Arc::new(VolcengineStreamingASR::new(credentials, vec![]));
    let partials = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&partials);
    asr.set_partial_transcript_callback(Some(Arc::new(move |text| {
        if !text.trim().is_empty() { count.fetch_add(1, Ordering::SeqCst); }
    })));
    asr.open_session().await.expect("new-key production connection failed");
    asr.mark_audio_delivery_ready();
    for chunk in pcm.chunks(3_200) {
        asr.consume_pcm_chunk(chunk);
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    let before_eof = partials.load(Ordering::SeqCst);
    asr.send_last_frame().await.unwrap();
    let result = asr.await_final_result().await.expect("new-key production final result failed");
    let chars = result.text.chars().count();
    asr.cancel();
    assert!(before_eof > 0, "streaming preview never appeared before EOF");
    assert!(chars >= 6, "synthetic recognition unexpectedly empty/short");
    println!("speech_api_key_live_probe={{\"partialUpdatesBeforeEof\":{before_eof},\"finalChars\":{chars},\"audioMs\":{},\"actualProductionClient\":true,\"microphoneOpened\":false}}", pcm.len() / 32);
}
