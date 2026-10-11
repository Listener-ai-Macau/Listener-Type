// Headless, opt-in recorded-public-input verification. This is not a live
// capture receipt, owner certificate, editor test or endpoint acceptance.
pub(super) fn run_recorded_content_probe() -> i32 {
    use crate::asr::AudioConsumer;
    use base64::{engine::general_purpose::STANDARD, Engine};
    use sha2::{Digest, Sha256};
    use std::io::{Read, Write};
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Probe {
        diagnostic_public_fixture: bool,
        source_sha256: String,
        reference_sha256: String,
        pcm_base64: String,
        reference_base64: String,
    }
    let result = (|| {
        let mut input = Vec::new();
        std::io::stdin()
            .lock()
            .take(1_048_577)
            .read_to_end(&mut input)
            .map_err(|e| e.to_string())?;
        if input.len() > 1_048_576 {
            return Err("content probe input oversized".into());
        }
        let probe: Probe =
            serde_json::from_slice(&input).map_err(|_| "invalid content probe input")?;
        if !probe.diagnostic_public_fixture {
            return Err("content probe requires an explicitly supplied public fixture".into());
        }
        let pcm = STANDARD
            .decode(probe.pcm_base64)
            .map_err(|_| "invalid content probe PCM")?;
        let reference = STANDARD
            .decode(probe.reference_base64)
            .map_err(|_| "invalid content probe reference")?;
        if !crate::asr::content_recovery_worker::valid_pcm(&pcm)
            || !crate::asr::content_recovery_worker::valid_pcm(&reference)
            || format!("{:x}", Sha256::digest(&pcm)) != probe.source_sha256
            || format!("{:x}", Sha256::digest(&reference)) != probe.reference_sha256
        {
            return Err("content probe fingerprint or complete input bounds mismatch".into());
        }
        let response = tauri::async_runtime::block_on(async move {
            let proxy = super::read_asr_proxy_config("volcengine").map_err(|e| e.to_string())?;
            let asr = Arc::new(
                VolcengineStreamingASR::new_with_proxy_config(
                    read_volc_credentials(),
                    vec![],
                    proxy,
                )
                .with_owner_activity_only(true)
                .with_content_recovery_locale("zh-CN".into()),
            );
            asr.open_session().await.map_err(|e| e.to_string())?;
            asr.mark_audio_delivery_ready();
            asr.consume_pcm_chunk(&pcm);
            asr.send_last_frame().await.map_err(|e| e.to_string())?;
            let primary = asr.await_final_result().await.map_err(|e| e.to_string())?;
            let original_text = primary.text.clone();
            // Diagnostic provenance is explicitly different from the real
            // direct wake-verifier receipt required by the coordinator.
            asr.set_verified_capture_content_reference(Some(reference));
            let supplement = asr.recover_content_supplement_once().await?;
            let diagnostic_supplement = supplement.clone();
            let decision = arbitrate_product_final_transcript(
                ProductFinalCandidates {
                    provider_primary: primary,
                    verified_provider_owner: None,
                    separated_owner: None,
                    retained_audio_replay: None,
                    debug_override: None,
                    partial_preview: None,
                    local_shadow: None,
                    local_shadow_owner_end_aligned: false,
                    content_supplement: supplement,
                    target_filter_required: false,
                    primary_mixture_rejected: false,
                    primary_speaker_filtered_certified: false,
                    separated_owner_exclusion_confirmed: false,
                },
                &[],
                false,
            );
            Ok::<_, String>(serde_json::json!({
                "sourceSha256":probe.source_sha256,"referenceSha256":probe.reference_sha256,
                "inputSamples":pcm.len()/2,"primaryText":original_text,"finalText":decision.transcript.text,
                "supplement":diagnostic_supplement,"supplementRecovered":decision.content_supplement_recovered,
                "primaryBytesRetained":decision.transcript.text.starts_with(&original_text),
                "physicalCaptureAccepted":false,"editorReceiptAccepted":false,"endpointAccepted":false,
                "referenceKind":"explicit-public-diagnostic-proxy","productVersion":env!("CARGO_PKG_VERSION")
            }))
        })?;
        let output = serde_json::to_vec(&response).map_err(|e| e.to_string())?;
        std::io::stdout()
            .lock()
            .write_all(&output)
            .map_err(|e| e.to_string())?;
        Ok::<_, String>(())
    })();
    match result {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("content probe: {error}");
            1
        }
    }
}
