//! Offline consumer regression. No inference, live observation time or product
//! identity grant is supplied by this frozen archived corpus.

use super::{CapturedWordAuthority, SourceWordBinding, WordIdentity};
use crate::speaker_channel_binding::{
    bind_verified_wake_owner_channel_samples, AnonymousSampleSpan, ExclusiveIdentityWindow,
    NativeWakeCoreSampleEvidence, ObservationDisposition, OwnerChannelBinder, OwnerChannelBinding,
    QualifiedIdentityEvidence, RetainedVerifiedWakeSampleAudio, SampleRange, SourceGeneration,
    VerifiedWakeOwnerSampleProof, WakeOwnerPolicy,
};
use crate::transcript_boundary::{
    consumed_prefix_end, content_key, TimedTranscript, TimedTranscriptWord,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant};

fn checked_bytes(asset: &Value) -> Vec<u8> {
    let bytes = std::fs::read(asset["path"].as_str().unwrap()).unwrap();
    assert_eq!(format!("{:x}", Sha256::digest(&bytes)), asset["sha256"]);
    bytes
}

fn checked_json(asset: &Value) -> Value {
    serde_json::from_slice(&checked_bytes(asset)).unwrap()
}

fn integer_sample(ms: &Value) -> u64 {
    let sample = ms.as_f64().unwrap() * 16.0;
    assert!(sample.is_finite() && sample >= 0.0 && (sample - sample.round()).abs() < 1e-6);
    sample.round() as u64
}

fn core_at(case: &Value, input: u64) -> Vec<AnonymousSampleSpan> {
    let report = checked_json(&case["core"]);
    let spans = if case["core_kind"] == "saved_actual_native_exact_samples" {
        assert_eq!(report["status"], "diagnostic_pass_not_production");
        assert_eq!(report["byte_exact_delivery"], true);
        assert_eq!(report["source_pcm_sha256"], case["pcm"]["sha256"]);
        assert_eq!(report["native_source_samples"].as_u64().unwrap(), input);
        report["native_core_samples"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| AnonymousSampleSpan {
                channel: c["channel"].as_u64().unwrap().try_into().unwrap(),
                range: SampleRange {
                    start: c["start"].as_u64().unwrap(),
                    end: c["end"].as_u64().unwrap(),
                },
            })
            .collect::<Vec<_>>()
    } else {
        assert_eq!(case["core_kind"], "saved_unpadded_native_core_ms");
        report
            .as_array()
            .unwrap()
            .iter()
            .map(|c| AnonymousSampleSpan {
                channel: c["speaker"].as_u64().unwrap().try_into().unwrap(),
                range: SampleRange {
                    start: integer_sample(&c["start_ms"]),
                    end: integer_sample(&c["end_ms"]),
                },
            })
            .collect()
    };
    spans
        .into_iter()
        .filter_map(|mut c| {
            // The saved last native frame can extend one frame beyond the physical
            // finish. It cannot lend source samples that were never captured.
            c.range.end = c.range.end.min(input);
            (c.range.start < c.range.end).then_some(c)
        })
        .collect()
}

fn protected_binding(case: &Value, source: SourceGeneration, input: u64) -> OwnerChannelBinding {
    let cadence = checked_json(&case["protected_cadence"]);
    let rows: Vec<_> = cadence["windows"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["source_id"] == case["id"])
        .collect();
    assert!(!rows.is_empty());
    let window = |row: &Value| ExclusiveIdentityWindow {
        channel: row["channel"].as_u64().unwrap().try_into().unwrap(),
        start_ms: row["start_ms"].as_u64().unwrap(),
        end_ms: row["end_ms"].as_u64().unwrap(),
    };
    let pcm_hash = if case["body_pcm"].is_object() {
        &case["body_pcm"]["sha256"]
    } else {
        &case["pcm"]["sha256"]
    };
    let mut binder = OwnerChannelBinder::new(source.generation, rows.iter().map(|r| window(r)));
    for row in rows {
        assert_eq!(&row["source_sha256"], pcm_hash);
        let prediction = &row["prediction"];
        assert_eq!(prediction["channel"], row["channel"]);
        assert_eq!(prediction["start_ms"], row["start_ms"]);
        assert_eq!(prediction["end_ms"], row["end_ms"]);
        let class = prediction["classification"].as_str().unwrap();
        let evidence = if prediction["signal_quality_sufficient"] == true {
            if class.starts_with("Target") {
                QualifiedIdentityEvidence::QualifiedTarget
            } else if class.starts_with("NonTarget") {
                QualifiedIdentityEvidence::QualifiedNonTarget
            } else {
                QualifiedIdentityEvidence::Pending
            }
        } else {
            QualifiedIdentityEvidence::Pending
        };
        assert_eq!(
            binder.observe(source.generation, window(row), input / 16, evidence),
            ObservationDisposition::Accepted
        );
    }
    binder.binding()
}

fn binding_at(
    case: &Value,
    source: SourceGeneration,
    pcm: &[u8],
    core: &[AnonymousSampleSpan],
) -> SourceWordBinding {
    let input = (pcm.len() / 2) as u64;
    if case["binding_kind"] == "completed_existing_protected_windows" {
        let binding = protected_binding(case, source, input);
        assert_eq!(
            binding,
            OwnerChannelBinding::Bound {
                channel: case["expected_bound_channel"].as_u64().unwrap() as u8
            }
        );
        return SourceWordBinding::from_qualified_channel(source, binding, 0, input).unwrap();
    }
    assert_eq!(case["binding_kind"], "byte_exact_existing_enrolled_wake");
    assert_eq!(
        protected_binding(case, source, input),
        OwnerChannelBinding::Pending
    );
    let verification = checked_bytes(&case["verification_pcm"]);
    let wake = checked_json(&case["wake_verification"]);
    for key in ["enrolled_policy", "matched", "owner_matched"] {
        assert_eq!(wake[key], true);
    }
    assert_eq!(wake["pcm_sha256"], case["verification_pcm"]["sha256"]);
    assert_eq!(&pcm[..verification.len()], verification.as_slice());
    let manifest = checked_json(&case["continuity_manifest"]);
    assert_eq!(manifest["verification"]["pcm_sha256"], wake["pcm_sha256"]);
    let body_origin = case["body_origin_sample"].as_u64().unwrap();
    assert_eq!(
        manifest["join"]["body_origin_ms"].as_u64().unwrap() * 16,
        body_origin
    );
    assert_eq!(
        &pcm[body_origin as usize * 2..],
        checked_bytes(&case["body_pcm"])
    );
    let native = checked_json(&case["core"]);
    let first_input = native["first_binding"]["input_sample"].as_u64().unwrap();
    let first_settled = native["first_binding"]["settled_through_sample"]
        .as_u64()
        .unwrap();
    let digest: [u8; 32] = Sha256::digest(&verification).into();
    let origin = case["declared_absolute_origin_sample"].as_u64().unwrap();
    let proof_range = SampleRange {
        start: origin,
        end: origin + (verification.len() / 2) as u64,
    };
    let proof = VerifiedWakeOwnerSampleProof {
        source,
        policy: WakeOwnerPolicy::EnrolledOwner,
        capture_range: proof_range,
        pcm_sha256: digest,
        observed_at_input_sample: case["verifier_observed_input_sample_assumption"]
            .as_u64()
            .unwrap(),
    };
    let retained = RetainedVerifiedWakeSampleAudio {
        source,
        capture_range: proof_range,
        continuous_range: SampleRange {
            start: 0,
            end: (verification.len() / 2) as u64,
        },
        pcm_sha256: digest,
        occurrence_count: 1,
        source_gap_free: true,
        body_origin_sample: body_origin,
    };
    let settled_core: Vec<_> = core
        .iter()
        .filter_map(|c| {
            let end = c.range.end.min(first_settled);
            (c.range.start < end).then_some(AnonymousSampleSpan {
                channel: c.channel,
                range: SampleRange {
                    start: c.range.start,
                    end,
                },
            })
        })
        .collect();
    let binding = bind_verified_wake_owner_channel_samples(
        proof,
        retained,
        NativeWakeCoreSampleEvidence {
            source,
            spans: &settled_core,
            settled_through_sample: first_settled,
            observed_at_input_sample: first_input,
        },
        first_input,
    )
    .unwrap();
    assert_eq!(
        binding.channel as u64,
        case["expected_bound_channel"].as_u64().unwrap()
    );
    assert_eq!(binding.valid_after_input_sample, first_input);
    SourceWordBinding::from_verified_wake(binding)
}

fn packet_timing(case: &Value) -> (TimedTranscript, Vec<TimedTranscriptWord>) {
    let report = checked_json(&case["provider"]);
    let pcm_hash = if case["body_pcm"].is_object() {
        &case["body_pcm"]["sha256"]
    } else {
        &case["pcm"]["sha256"]
    };
    assert_eq!(&report["pcm_sha256"], pcm_hash);
    let response =
        &report["provider_responses"][case["provider_response_index"].as_u64().unwrap() as usize];
    let all_words: Vec<_> = response["result"]["utterances"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|row| row["words"].as_array().unwrap())
        .map(|w| TimedTranscriptWord {
            text: w["text"].as_str().unwrap().into(),
            start_ms: w["start_time"].as_u64(),
            end_ms: w["end_time"].as_u64(),
        })
        .collect();
    let start = case["word_start_index"].as_u64().unwrap() as usize;
    let expected = case["words"].as_array().unwrap();
    let words = all_words[start..start + expected.len()].to_vec();
    for (w, e) in words.iter().zip(expected) {
        assert_eq!(w.text, e["text"].as_str().unwrap());
        assert_eq!(w.start_ms, e["start_ms"].as_u64());
        assert_eq!(w.end_ms, e["end_ms"].as_u64());
    }
    let prefix = |end| {
        all_words[..end]
            .iter()
            .map(|w| content_key(&w.text))
            .collect::<String>()
    };
    let text = response["result"]["text"].as_str().unwrap();
    assert_eq!(content_key(text), prefix(all_words.len()));
    let from = consumed_prefix_end(text, &prefix(start)).unwrap();
    let through = consumed_prefix_end(text, &prefix(start + expected.len())).unwrap();
    (
        TimedTranscript {
            text: text[from..through].into(),
            words,
        },
        all_words,
    )
}

#[test]
#[ignore = "requires frozen archived source, protected-bank and actual provider word evidence"]
fn pinned_archived_source_words_share_preview_live_and_final_authority() {
    let fixture: Value = serde_json::from_slice(
        &std::fs::read(std::env::var_os("LISTENER_CAPTURED_WORD_CORPUS").unwrap()).unwrap(),
    )
    .unwrap();
    assert_eq!(
        fixture["schema"],
        "listener.captured-source-word-consumer.inputs.v1"
    );
    let protocol = checked_json(&fixture["protocol"]);
    for kind in ["code", "evidence"] {
        for file in protocol[kind].as_array().unwrap() {
            checked_bytes(file);
        }
    }
    let receipt = Instant::now();
    let decision = receipt + Duration::from_secs(1);
    let mut reports = Vec::new();
    let mut quiet_owner = 0;
    for case in fixture["cases"].as_array().unwrap() {
        let ids = &case["source"];
        let source = SourceGeneration {
            source_id: ids["source_id"].as_u64().unwrap(),
            capture_id: ids["capture_id"].as_u64().unwrap(),
            generation: ids["generation"].as_u64().unwrap(),
        };
        let pcm = checked_bytes(&case["pcm"]);
        assert_eq!(pcm.len() % 2, 0);
        let input = (pcm.len() / 2) as u64;
        let core = core_at(case, input);
        let binding = binding_at(case, source, &pcm, &core);
        let (timing, packet_words) = packet_timing(case);
        let mut ledger = CapturedWordAuthority::new(source);
        ledger
            .observe_native(source, binding, &core, input, input, receipt)
            .unwrap();
        let identities: Vec<_> = timing
            .words
            .iter()
            .map(|w| {
                ledger.identity(
                    source,
                    w.start_ms.unwrap(),
                    w.end_ms.unwrap(),
                    input,
                    decision,
                )
            })
            .collect();
        let owner = identities
            .iter()
            .filter(|i| **i == WordIdentity::Owner)
            .count();
        let other = identities
            .iter()
            .filter(|i| **i == WordIdentity::Other)
            .count();
        let pending = identities
            .iter()
            .filter(|i| **i == WordIdentity::Pending)
            .count();
        assert_eq!(owner as u64, case["expected"]["owner"].as_u64().unwrap());
        assert_eq!(other as u64, case["expected"]["other"].as_u64().unwrap());
        assert_eq!(
            pending as u64,
            case["expected"]["pending"].as_u64().unwrap()
        );
        let visual = ledger.project(source, &timing, true, input, decision);
        let live = ledger.project(source, &timing, false, input, decision);
        let final_view = ledger.project(source, &timing, false, input, decision);
        assert_eq!(visual.timing.text, timing.text);
        assert_eq!(live.timing.text, final_view.timing.text);
        let signature = |timing: &TimedTranscript| {
            timing
                .words
                .iter()
                .map(|w| (w.text.clone(), w.start_ms, w.end_ms))
                .collect::<Vec<_>>()
        };
        assert_eq!(signature(&live.timing), signature(&final_view.timing));
        assert_eq!(live.updated_at, final_view.updated_at);
        assert_eq!(live.updated_at, Some(receipt));
        assert!(!visual.excluded_other && !live.excluded_other && !final_view.excluded_other);
        assert_eq!(live.unresolved, pending > 0);
        assert_eq!(
            live.timing.words.len(),
            identities
                .iter()
                .take_while(|i| **i == WordIdentity::Owner)
                .count()
        );
        assert!(ledger
            .project(
                source,
                &timing,
                false,
                input,
                receipt - Duration::from_millis(1)
            )
            .timing
            .text
            .is_empty());
        let stale = SourceGeneration {
            capture_id: source.capture_id + 1,
            ..source
        };
        assert!(ledger
            .project(stale, &timing, false, input, decision)
            .timing
            .text
            .is_empty());
        if let Some(foreign) = case["foreign_words"].as_array() {
            for w in foreign {
                let word = TimedTranscriptWord {
                    text: w["text"].as_str().unwrap().into(),
                    start_ms: w["start_ms"].as_u64(),
                    end_ms: w["end_ms"].as_u64(),
                };
                assert!(packet_words.iter().any(|w| w.text == word.text
                    && w.start_ms == word.start_ms
                    && w.end_ms == word.end_ms));
                assert_eq!(
                    ledger.identity(
                        source,
                        word.start_ms.unwrap(),
                        word.end_ms.unwrap(),
                        input,
                        decision
                    ),
                    WordIdentity::Pending
                );
            }
        } else {
            quiet_owner += owner;
            assert_eq!(live.timing.text, timing.text);
        }
        if case["id"] == "quiet-receipt-resume" {
            for boundary in protocol["boundary_regression"].as_array().unwrap() {
                let position = case["words"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .position(|w| w == boundary)
                    .unwrap();
                assert_eq!(identities[position], WordIdentity::Owner);
            }
        }
        reports.push(json!({"id": case["id"], "binding_kind": case["binding_kind"], "owner": owner, "other": other, "pending": pending, "live_final_prefix_word_count": live.timing.words.len(), "visual_body_retained": true, "foreign_pending_word_count": case["foreign_words"].as_array().map_or(0, Vec::len), "body_origin_sample": case["body_origin_sample"]}));
    }
    assert_eq!(quiet_owner, 132);
    let result = json!({
        "schema": "listener.captured-source-word-consumer.result.v1",
        "status": "diagnostic_pass_not_production", "protocol_sha256": fixture["protocol"]["sha256"],
        "quiet_owner_words": quiet_owner, "cases": reports, "boundaries_preserved": protocol["boundary_regression"],
        "limitations": protocol["forbidden_claims"], "production_connected": false, "new_model_or_asr_inference": false,
    });
    std::fs::write(
        std::env::var_os("LISTENER_CAPTURED_WORD_RESULT").unwrap(),
        serde_json::to_vec_pretty(&result).unwrap(),
    )
    .unwrap();
    println!("Captured source consumer:132 quiet Owner words;17 drama Owner candidates and27 Pending. No acoustic/live acceptance.");
}
