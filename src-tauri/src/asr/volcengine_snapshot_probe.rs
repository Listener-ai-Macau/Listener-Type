// A diagnostic for the lexical transport boundary. It deliberately creates no
// voiceprint, separator, preview callback, editor receipt or physical STOP.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "uses an explicitly supplied saved recording and live provider"]
async fn recorded_lexical_snapshot_probe() {
    use crate::persistence::{CredentialAccount, CredentialsVault};
    use sha2::{Digest, Sha256};
    let input = std::env::var("LISTENER_LEXICAL_SNAPSHOT_INPUT").unwrap();
    let output = std::env::var("LISTENER_LEXICAL_SNAPSHOT_OUTPUT").unwrap();
    let fixture: Value = serde_json::from_slice(&std::fs::read(input).unwrap()).unwrap();
    let mut pcm = std::fs::read(fixture["pcm_path"].as_str().unwrap()).unwrap();
    assert_eq!(format!("{:x}", Sha256::digest(&pcm)), fixture["pcm_sha256"].as_str().unwrap().to_ascii_lowercase());
    let through_ms = fixture["snapshot_end_ms"].as_u64().unwrap();
    assert!(through_ms > 0 && through_ms <= 60_000 && through_ms * 32 <= pcm.len() as u64);
    pcm.truncate(through_ms as usize * 32);
    let credential = |account| CredentialsVault::get(account).unwrap().filter(|v| !v.is_empty()).unwrap();
    let asr = Arc::new(VolcengineStreamingASR::new(VolcengineCredentials {
        app_id: credential(CredentialAccount::VolcengineAppKey),
        access_token: credential(CredentialAccount::VolcengineAccessKey),
        resource_id: credential(CredentialAccount::VolcengineResourceId),
    }, Vec::new()));
    let began = Instant::now();
    asr.open_session().await.unwrap();
    let connected_ms = began.elapsed().as_millis();
    asr.mark_audio_delivery_ready();
    AudioConsumer::consume_pcm_chunk(&*asr, &pcm);
    asr.send_last_frame().await.unwrap();
    let final_result = asr.await_final_result().await.unwrap();
    let elapsed_ms = began.elapsed().as_millis();
    let state = asr.state.lock();
    let timing = state.provider_timed_transcript.as_ref().unwrap();
    let confirmed = state.recognition_boundary.confirmed_prefix(&timing.text, Some(timing));
    let report = json!({
        "scope": "Lexical-only saved-audio snapshot; no ownership or delivery authority",
        "captured_through_ms": through_ms,
        "connection_ms": connected_ms,
        "total_ms": elapsed_ms,
        "raw_final": final_result.text,
        "timed_text": timing.text,
        "confirmed_text": confirmed,
        "clause_finality_advertised": state.recognition_boundary.requires_confirmation(),
        "words": timing.words.iter().map(|word| json!({"text": word.text, "start_ms": word.start_ms, "end_ms": word.end_ms})).collect::<Vec<_>>(),
        "responses": state.diagnostic_provider_responses,
        "release_accepted": false,
    });
    std::fs::write(output, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    assert!(state.recognition_boundary.requires_confirmation());
    assert!(confirmed.is_some());
}
