//! Candidate sample-accurate source evidence. Masked audio supplies identity
//! features only; source support, raw positives and native owner ranges own
//! separate coordinates. This module is not connected to physical STOP.
use super::tracked_source_presence::{SourceEpoch, TrackedSourceDecision, TrackedSourcePresence};
use super::volcengine_speaker_ledger::{EvidenceKind, LocalSpeakerTimeline, SpeakerSpan};
use crate::personal_vad::{PresenceObservation, PresenceState};
use crate::speaker_verification::{NativeWordSpeakerProfile, SessionSpeakerObservation};
use ort::{session::Session, value::TensorRef};
use sha2::{Digest, Sha256};
use std::{path::Path, time::Instant};

const MODEL_SHA: &str = "220ad67ca923bef2fa91f2390c786097bf305bceb5e261d4af67b38e938e1079";
const CLASSES: [[bool; 3]; 7] = [[false,false,false],[true,false,false],[false,true,false],
    [false,false,true],[true,true,false],[true,false,true],[false,true,true]];

#[derive(Clone)]
pub(crate) struct BoundSourceEvidence {
    // Source identity belongs to this PCM capture and protected profile. Text
    // delivery stability generations are deliberately not part of its lifetime:
    // a preview/authority revision is not a new speaker or a new microphone.
    pub epoch: SourceEpoch,
    pub phrase: String,
    pub profile_key: [u8;32],
    pub captured_ms: u64,
    pub source_analyzed_end_ms: u64,
    pub frame: PresenceObservation,
    pub timeline: LocalSpeakerTimeline,
    pub raw: LocalSpeakerTimeline,
    pub raw_analyzed_end_ms: Option<u64>,
    pub owner_ids: Vec<String>,
    pub foreign_ids: Vec<String>,
    pub metadata_revision:u64,
    pub metadata_clock:std::sync::Arc<std::sync::atomic::AtomicU64>,
    #[cfg(test)]
    pub diagnostic: serde_json::Value,
}

impl BoundSourceEvidence {
    pub(crate) fn common_completed_frame(mut frame: PresenceObservation, raw_end_ms: Option<u64>) -> Option<PresenceObservation> {
        // Source support and the personal detector have different quanta. A
        // clock may claim only their intersection, without erasing any Owner
        // or unresolved veto observed beyond that shared frontier.
        frame.analyzed_end_ms = frame.analyzed_end_ms.min(raw_end_ms?);
        Some(frame)
    }

    pub(crate) fn raw_review_is_current(&self, current: &crate::personal_vad::RawActivitySnapshot) -> bool {
        self.raw_analyzed_end_ms.is_some_and(|end| end >= self.frame.analyzed_end_ms)
            && current.completed_through_ms().is_some_and(|end| end >= self.frame.analyzed_end_ms)
            && !self.has_unreviewed_raw_owner(&current.timeline, 0, current.completed_through_ms().unwrap_or_default())
    }

    /// A certificate may outlive its clock frame, but cannot ignore a raw
    /// positive that arrived after the source producer reconciled this range.
    pub(crate) fn has_unreviewed_raw_owner(&self, current: &LocalSpeakerTimeline, start: u64, end: u64) -> bool {
        for span in current.presence.iter().filter(|span| span.kind == EvidenceKind::Owner
            && span.start_ms < end && start < span.end_ms) {
            let first = start.max(span.start_ms);
            let last = end.min(span.end_ms);
            let mut covered = first;
            let mut previous: Vec<_> = self.raw.presence.iter().filter(|old|old.kind == EvidenceKind::Owner
                && old.start_ms < last && first < old.end_ms).collect();
            previous.sort_by_key(|span|span.start_ms);
            for old in previous {
                if old.start_ms > covered { break; }
                covered = covered.max(old.end_ms);
                if covered >= last { break; }
            }
            if covered < last { return true; }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asr::volcengine::{VolcengineStreamingASR,LiveBodyConsumption};
    use crate::transcript_boundary::{content_key,TimedTranscript};
    use crate::asr::AudioConsumer;
    use serde_json::{Value,json};
    use std::{time::Duration,sync::Arc};

    #[test]
    fn source_absence_waits_for_personal_activity_completion_and_rechecks_late_owner() {
        let mut evidence = BoundSourceEvidence {
            epoch:SourceEpoch {capture:1,capture_generation:1,decoder:1},
            phrase:"test".into(),profile_key:[0;32],captured_ms:4000,source_analyzed_end_ms:4000,
            frame:PresenceObservation {analyzed_end_ms:4000,last_positive_end_ms:Some(1000),
                last_unresolved_end_ms:None,source_history_complete:true,
                state:PresenceState::ConfirmedOther,observed_at:Instant::now()},
            timeline:LocalSpeakerTimeline::default(),raw:LocalSpeakerTimeline::default(),
            raw_analyzed_end_ms:Some(2560),owner_ids:Vec::new(),foreign_ids:Vec::new(),
            metadata_revision:0,metadata_clock:Arc::new(std::sync::atomic::AtomicU64::new(0)),
            diagnostic:json!({}),
        };
        let mut raw = crate::personal_vad::RawActivitySnapshot {
            timeline:LocalSpeakerTimeline::default(),analyzed_end_samples:Some(2560*16),..Default::default()
        };
        assert!(!evidence.raw_review_is_current(&raw), "source actor ahead is not personal-model absence");
        raw.analyzed_end_samples = Some(4000*16);
        assert!(!evidence.raw_review_is_current(&raw), "an old source snapshot must be reconciled after catch-up");
        evidence.raw_analyzed_end_ms = Some(4000);
        assert!(evidence.raw_review_is_current(&raw));
        raw.timeline.note_presence(SpeakerSpan {start_ms:3200,end_ms:3360,kind:EvidenceKind::Owner});
        assert!(!evidence.raw_review_is_current(&raw), "late original-audio owner veto invalidates absence");
        evidence.raw = raw.timeline.clone();
        assert!(evidence.raw_review_is_current(&raw), "source fusion may reconsider only after reviewing the new veto");
        evidence.raw_analyzed_end_ms = None;
        assert!(!evidence.raw_review_is_current(&raw), "missing completion cannot be inferred from capture");
    }

    #[test]
    #[ignore="actual saved failed PCM through Rust ONNX and shared pending-body consumer; no socket/editor/STOP"]
    fn recorded_rust_source_evidence_separates_foreign_body_obligation_without_fabricating_receipts() {
        let root=std::path::PathBuf::from(std::env::var_os("LISTENER_COMPLETE_SOURCE_REPLAY").unwrap());
        let selected=std::env::var("LISTENER_COMPLETE_SOURCE_CASE").unwrap();
        let protocol_file=if selected.starts_with("late-"){"source-protocol-late.json"}else{"source-protocol.json"};
        let protocol:Value=serde_json::from_slice(&std::fs::read(root.join(protocol_file)).unwrap()).unwrap();
        let case=protocol["cases"].as_array().unwrap().iter().find(|c|c["id"]==selected).unwrap();
        let phrase=protocol["phrase"].as_str().unwrap();
        let profile=crate::speaker_verification::native_word_profile_for_phrase(phrase).unwrap().unwrap();
        let pcm=std::fs::read(case["pcm"].as_str().unwrap()).unwrap();
        assert_eq!(format!("{:x}",Sha256::digest(&pcm)),case["pcmSha256"].as_str().unwrap());
        let native:Value=serde_json::from_slice(&std::fs::read(case["native"].as_str().unwrap()).unwrap()).unwrap();
        let nomo:Value=serde_json::from_slice(&std::fs::read(case["nomo"].as_str().unwrap()).unwrap()).unwrap();
        let mut packets:Vec<_>=native["frames"].as_array().unwrap().iter().map(|f|(1_u64,f.clone())).collect();
        let prefix=root.join(format!("{}-prefix-native.json",selected));
        let network_prefix=std::env::var_os("LISTENER_SOURCE_PREFIX_NETWORK_REPLAY").is_some();
        let recorded_prefix=std::env::var_os("LISTENER_SOURCE_PREFIX_FIXED_REPLAY").map(|path| {
            let fixture:Value=serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
            assert_eq!(format!("{:x}",Sha256::digest(&pcm)),fixture["fullPcmSha256"].as_str().unwrap());
            let receipts=fixture["receipts"].as_array().cloned().unwrap_or_else(||vec![fixture]);
            assert!(receipts.windows(2).all(|pair|pair[0]["capturedMsAtReceipt"].as_u64()<=pair[1]["capturedMsAtReceipt"].as_u64()),"saved prefix receipts out of order");
            for receipt in &receipts {
                assert_eq!(receipt["fullPcmSha256"],case["pcmSha256"]);
                assert!(receipt["capturedThroughMs"].as_u64().unwrap()<=receipt["capturedMsAtReceipt"].as_u64().unwrap());
            }
            receipts
        });
        if prefix.exists() && !network_prefix {
            let prefix:Value=serde_json::from_slice(&std::fs::read(prefix).unwrap()).unwrap();
            packets.extend(prefix["frames"].as_array().unwrap().iter().map(|f|(2_u64,f.clone())));
        }
        packets.sort_by(|a,b|a.1["capturedMsAtReceipt"].as_f64().unwrap().total_cmp(&b.1["capturedMsAtReceipt"].as_f64().unwrap()));
        let asr=Arc::new(VolcengineStreamingASR::source_replay_capture_for_test(phrase));
        let epoch=asr.source_replay_epoch_for_test();
        let queued=std::env::var_os("LISTENER_QUEUED_SOURCE_REPLAY").is_some();
        assert!(!network_prefix || queued,"prefix service requires real queue cadence");
        let integrated=std::env::var_os("LISTENER_SOURCE_INTEGRATED_REPLAY").is_some();
        assert!(!integrated || network_prefix,"integrated receiver needs a runtime");
        let prefix_runtime=network_prefix.then(||tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap());
        let native_sender=integrated.then(|| {
            let (tx,mut rx)=tokio::sync::mpsc::channel::<Value>(256);let asr=Arc::clone(&asr);
            prefix_runtime.as_ref().unwrap().spawn(async move{while let Some(packet)=rx.recv().await{asr.apply_saved_native_through_receiver_for_test(&packet).await;}});
            tx
        });
        let mut tracker=TrackedSourcePresence::new(epoch,phrase,&profile);
        let model=root.parent().unwrap().join("public-segmentation/sherpa-onnx-pyannote-segmentation-3-0/model.onnx");
        let mut audio=(!queued).then(||TrackedSourceAudio::load(&model,epoch).unwrap());
        if integrated{asr.start_personal_vad(None,phrase);assert!(asr.personal_vad_started());}
        let worker_raw=if integrated{asr.source_raw_activity_for_test()}else{Arc::new(parking_lot::Mutex::new(crate::personal_vad::RawActivitySnapshot::default()))};
        let mut worker:Option<Arc<super::super::tracked_source_worker::TrackedSourceWorker>>=None;
        let mut raw=LocalSpeakerTimeline::default();let mut pi=0;let mut ni=0;let mut delivered_pcm=0;
        let mut bookmark=None;let mut reports=Vec::new();let mut reader_compute=Vec::new();
        let mut source_clock=crate::personal_vad::PresenceEndpoint::default();
        let mut hypothetical_receipt_clock=crate::personal_vad::PresenceEndpoint::default();
        let mut modeled_receipt_attached=false;
        let scores=nomo["windows"].as_array().unwrap();
        let mut latest_main=json!({});
        let mut frames_without_bookmark=0;
        let wall_start=Instant::now();let first_step=case["steps"][0]["capturedMs"].as_u64().unwrap();
        let origin_ms=if integrated{0}else{first_step};
        let capture_thread=if integrated {
            let source_worker=Arc::new(super::super::tracked_source_worker::TrackedSourceWorker::start(epoch,phrase.into(),profile.clone(),
                Vec::new(),model.clone(),Arc::clone(&worker_raw),asr.source_evidence_sink_for_test()));
            asr.enable_source_prefix_service_for_test(prefix_runtime.as_ref().unwrap().handle().clone(),Arc::clone(&source_worker));
            worker=Some(Arc::clone(&source_worker));
            let captured_asr=Arc::clone(&asr);let audio_pcm=pcm.clone();let native=packets.clone();let sender=native_sender.as_ref().unwrap().clone();
            let recorded=recorded_prefix.clone();let runtime=prefix_runtime.as_ref().unwrap().handle().clone();
            Some(std::thread::spawn(move || {
                let mut offset=0;let mut next_native=0;let mut next_recorded=0;
                while offset<audio_pcm.len() {
                    let end=(offset+40*32).min(audio_pcm.len());
                    let deadline=wall_start+Duration::from_millis(end as u64/32);
                    if deadline>Instant::now(){std::thread::sleep(deadline.saturating_duration_since(Instant::now()));}
                    captured_asr.consume_pcm_chunk(&audio_pcm[offset..end]);
                    captured_asr.submit_personal_vad(&audio_pcm[offset..end],end as u64/2);
                    source_worker.submit_audio(&audio_pcm[offset..end],end as u64/2);offset=end;
                    while next_native<native.len() && native[next_native].1["capturedMsAtReceipt"].as_f64().unwrap()<=end as f64/32.0 {
                        sender.try_send(native[next_native].1["provider_result"].clone()).unwrap();next_native+=1;
                    }
                    if let Some(receipts)=recorded.as_ref() {
                        while let Some(fixture)=receipts.get(next_recorded).filter(|f|f["capturedMsAtReceipt"].as_u64().unwrap()<=end as u64/32) {
                            next_recorded+=1;
                            let parent=Arc::clone(&captured_asr);let worker=Arc::clone(&source_worker);let fixture=fixture.clone();
                            runtime.block_on(parent.apply_recorded_tracked_prefix_for_test(&fixture,worker));
                        }
                    }
                }
            }))
        }else{None};
        // The product watchdog polls every 50ms independently of PCM ingress.
        // Sampling only immediately after a 480ms audio batch falsely rejects
        // every still-computing snapshot. Keep capture, receiver and reads independent.
        let reading_steps:Vec<Value>=if integrated{(first_step..pcm.len() as u64/32).step_by(50).map(|ms|json!({"capturedMs":ms})).collect()}
            else{case["steps"].as_array().unwrap().clone()};
        for step in &reading_steps {
            let scheduled=step["capturedMs"].as_u64().unwrap();
            if queued {
                let deadline=wall_start+Duration::from_millis(scheduled-origin_ms);
                if deadline>Instant::now(){std::thread::sleep(deadline.saturating_duration_since(Instant::now()));}
            }
            let captured=if integrated{asr.source_captured_ms_for_test()}else{scheduled};let bytes=captured as usize*32;
            if !integrated{asr.consume_pcm_chunk(&pcm[delivered_pcm..bytes]);delivered_pcm=bytes;}
            while !integrated && pi<scores.len() && scores[pi]["endMs"].as_u64().unwrap()<=captured {
                let end=scores[pi]["endMs"].as_u64().unwrap();
                for (i,p) in scores[pi]["probabilities"].as_array().unwrap().iter().enumerate() {
                    let finish=end-(14-i) as u64*160;
                    if pi==0 || i==14 {raw.note_presence(SpeakerSpan{start_ms:finish-160,end_ms:finish,kind:EvidenceKind::Unknown});}
                    if p.as_f64().unwrap()>=0.7 {raw.note_presence(SpeakerSpan{start_ms:finish-160,end_ms:finish,kind:EvidenceKind::Owner});}
                }
                pi+=1;
            }
            if queued && !integrated {
                *worker_raw.lock()=crate::personal_vad::RawActivitySnapshot { timeline:raw.clone(),
                    analyzed_end_samples:pi.checked_sub(1).map(|i|scores[i]["endMs"].as_u64().unwrap()*16),..Default::default() };
                if let Some(worker)=worker.as_ref(){super::super::tracked_source_worker::TrackedSourceWorker::submit_audio(worker,&pcm[bytes-480*32..bytes],bytes as u64/2);}
                else {
                    let source_worker=Arc::new(super::super::tracked_source_worker::TrackedSourceWorker::start(epoch,phrase.into(),profile.clone(),
                        pcm[..bytes].to_vec(),model.clone(),Arc::clone(&worker_raw),asr.source_evidence_sink_for_test()));
                    if let Some(runtime)=prefix_runtime.as_ref(){asr.enable_source_prefix_service_for_test(runtime.handle().clone(),Arc::clone(&source_worker));}
                    worker=Some(source_worker);
                }
            }
            while !integrated && ni<packets.len() && packets[ni].1["capturedMsAtReceipt"].as_f64().unwrap()<=captured as f64 {
                let (decoder,packet)=&packets[ni];
                if !queued {tracker.observe_native(epoch,*decoder,&packet["provider_result"],&pcm[..bytes],&raw,&profile);}
                if !integrated{if let Some(worker)=worker.as_ref(){worker.submit_native(*decoder,packet["provider_result"].clone());}}
                if *decoder==1 {
                    if let Some(sender)=native_sender.as_ref(){sender.try_send(packet["provider_result"].clone()).unwrap();}
                    else{latest_main=packet["provider_result"].clone();asr.apply_saved_native_result_for_test(&latest_main,false);}
                }
                ni+=1;
            }
            if integrated{latest_main=asr.source_native_result_for_test().unwrap_or_else(||json!({}));}
            if bookmark.is_none() {
                if let Some(_row)=latest_main["utterances"].as_array().into_iter().flatten().find(|r|r["definite"]==true && content_key(r["text"].as_str().unwrap_or_default()).starts_with(&content_key(phrase))) {
                    if let Some(snapshot)=asr.pause_early_delivery_evidence_after(Duration::ZERO,"") {
                        if let Some(timing)=snapshot.timing {
                            let confirmed_text=latest_main["utterances"].as_array().unwrap().iter().take_while(|r|r["definite"]==true)
                                .map(|r|r["text"].as_str().unwrap()).collect::<String>();
                            let key=content_key(&confirmed_text);let mut built=String::new();let mut words=Vec::new();
                            for word in timing.timing.words {
                                let token=content_key(&word.text);if token.is_empty(){continue;}
                                if !key[built.len()..].starts_with(&token){break;}
                                built.push_str(&token);words.push(word);if built==key{break;}
                            }
                            if built==key {
                                let timed=TimedTranscript{text:confirmed_text,words};
                                bookmark=Some(LiveBodyConsumption {raw_prefix:timed.text.clone(),body_start:0,rendered_key:key.clone(),
                                    source:snapshot.source,boundary:timed.delivery_boundary(&key)});
                            }
                        }
                    }
                }
            }
            if !queued{asr.install_source_evidence_for_test(None);}
            let before=bookmark.as_ref().and_then(|b|if queued{asr.pending_without_source_resolution_for_test(b)}else{asr.pending_body_delivery_from_consumption(b)});
            let began=Instant::now();
            if let Some(audio)=audio.as_mut(){audio.analyze(epoch,&pcm[..bytes],&profile).unwrap();}
            let direct=audio.as_ref().map(|audio|audio.evidence(epoch,&tracker,&profile,&raw,Instant::now()).unwrap());
            reader_compute.push(began.elapsed().as_millis());
            let (frame,timeline)=if queued {
                let began=Instant::now();
                while !integrated && captured>=22080 && began.elapsed()<Duration::from_millis(220)
                    && asr.personal_vad_snapshot().is_none(){std::thread::sleep(Duration::from_millis(20));}
                let frame=asr.personal_vad_snapshot();let sink=asr.source_evidence_sink_for_test();let evidence=sink.lock();
                (frame,evidence.as_ref().map(|e|e.timeline.clone()).unwrap_or_default())
            }else if let Some((frame,timeline))=direct {
                asr.install_source_evidence_for_test(Some(BoundSourceEvidence {epoch,phrase:phrase.into(),
                    profile_key:profile.binding_key(),captured_ms:captured,source_analyzed_end_ms:frame.analyzed_end_ms,frame,timeline:timeline.clone(),raw:raw.clone(),
                    raw_analyzed_end_ms:pi.checked_sub(1).map(|i|scores[i]["endMs"].as_u64().unwrap()),owner_ids:tracker.owner_ids_for(1),
                    foreign_ids:tracker.foreign_ids_for(1),metadata_revision:0,metadata_clock:Arc::new(std::sync::atomic::AtomicU64::new(0)),
                    diagnostic:audio.as_ref().unwrap().diagnostic(&tracker,&raw,&timeline)}));(Some(frame),timeline)
            }else{(None,LocalSpeakerTimeline::default())};
            let captured=if integrated{asr.source_captured_ms_for_test()}else{captured};
            let after=bookmark.as_ref().and_then(|b|asr.pending_body_delivery_from_consumption(b));
            if network_prefix && recorded_prefix.is_none(){asr.poll_tracked_source_jobs();}
            // The fixed-bookmark comparison above isolates foreign obligation
            // resolution. Separately exercise the real confirmed delivery
            // selector, with an explicitly hypothetical successful consume.
            // No editor receipt, deadline or physical STOP is fabricated.
            let mut hypothetical_delivery=None;
            if network_prefix {
                if let Some(previous)=bookmark.as_ref() {
                    if let Some(snapshot)=asr.pause_early_delivery_evidence_after(Duration::ZERO,&previous.rendered_key) {
                        if snapshot.stability==super::super::volcengine::LiveDeliveryStability::ProviderConfirmed {
                            if let Some(timing)=snapshot.timing {
                                let key=content_key(&snapshot.text);
                                if !key.is_empty() && key!=previous.rendered_key {
                                    let boundary=timing.timing.delivery_boundary(&key);
                                    if boundary.as_ref().zip(previous.boundary.as_ref()).is_some_and(|(next,old)|next.range_ms().1>old.range_ms().1) {
                                        hypothetical_delivery=Some(json!({"capturedMs":captured,"decoder":snapshot.source.decoder_stream_id,
                                            "text":snapshot.text,"actualEditor":false,"actualReceipt":false}));
                                        bookmark=Some(LiveBodyConsumption{raw_prefix:snapshot.text,body_start:0,rendered_key:key,source:snapshot.source,boundary});
                                    }
                                }
                            }
                        }
                    }
                }
            }
            let pending_after_hypothetical_delivery=bookmark.as_ref().and_then(|b|asr.pending_body_delivery_from_consumption(b));
            if let Some(frame)=frame{source_clock.observe(frame);}
            if let Some(frame)=frame{hypothetical_receipt_clock.observe(frame);}
            if !modeled_receipt_attached && bookmark.as_ref().is_some_and(|b|content_key(&b.raw_prefix)
                .strip_prefix(&content_key(phrase)).is_some_and(|body|!body.is_empty())) {
                hypothetical_receipt_clock.note_delivery(Instant::now());modeled_receipt_attached=true;
            }
            if hypothetical_delivery.is_some(){hypothetical_receipt_clock.note_delivery(Instant::now());}
            let source_due=source_clock.due(captured,Instant::now(),3000);
            let hypothetical_receipt_stop=modeled_receipt_attached
                && hypothetical_receipt_clock.due(captured,Instant::now(),3000)
                && pending_after_hypothetical_delivery.is_none();
            let return_range=case["ownerReturnRangeMsAssessmentOnly"].as_array();
            // Assessments only assert the source contract. They do not choose
            // references, replace cloud words, or invent an editor receipt.
            if return_range.is_some_and(|r|captured>=r[0].as_u64().unwrap() && captured<=r[1].as_u64().unwrap()) {
                assert!(!source_due,"source endpoint became eligible while saved owner continuation was still speaking");
            }
            if bookmark.is_none(){frames_without_bookmark+=1;}
            let suffix_ranges=latest_main["utterances"].as_array().into_iter().flatten().flat_map(|r|r["words"].as_array().into_iter().flatten())
                .filter_map(|w|w["start_time"].as_u64().zip(w["end_time"].as_u64())).collect::<Vec<_>>();
            reports.push(json!({"capturedMs":captured,"sourceAnalyzedMs":frame.map(|f|f.analyzed_end_ms),
                "state":frame.map(|f|format!("{:?}",f.state)),"sourceEligibleWithoutReceipt":source_due,
                "hypotheticalReceiptStopEligible":hypothetical_receipt_stop,"actualReceipt":false,
                "ownerFrontierMs":frame.and_then(|f|f.last_positive_end_ms),
                "unknownFrontierMs":frame.and_then(|f|f.last_unresolved_end_ms),"references":if queued{None}else{Some(tracker.reference_counts())},
                "pendingBefore":before.as_ref().map(|b|json!({"key":b.key,"recognitionPending":b.recognition_pending})),
                "pendingAfter":after.as_ref().map(|b|json!({"key":b.key,"recognitionPending":b.recognition_pending})),
                "bookmarkIsHypotheticalSuccessfulConsumption":bookmark.is_some(),
                "pendingScope":bookmark.as_ref().map(|b|asr.tracked_pending_scope_for_test(b)),
                "sourceEvidence":if queued{asr.source_evidence_sink_for_test().lock().as_ref().map(|e|e.diagnostic.clone())}else{None},
                "hypotheticalConfirmedDelivery":hypothetical_delivery,
                "pendingAfterHypotheticalConfirmedDelivery":pending_after_hypothetical_delivery.as_ref().map(|b|json!({"key":b.key,"recognitionPending":b.recognition_pending})),
                "prefixService":network_prefix.then(||asr.source_prefix_service_scope_for_test()),
                "nativeWordsWithCompleteForeignEvidence":suffix_ranges.iter().filter(|(a,b)|timeline.confirms_other(*a,*b)).count()}));
        }
        let released=reports.iter().filter(|r|!r["pendingBefore"].is_null() && r["pendingAfter"].is_null()).count();
        std::fs::write(root.join(format!("{}-{}.json",selected,if queued{"queued-source-result"}else{"rust-source-refined-result"})),serde_json::to_vec_pretty(&json!({
            "case":selected,"newlyReleasedForeignBodyObligations":released,"framesWithoutBookmark":frames_without_bookmark,
            "actualEditor":false,"actualStop":false,"editorReceiptsFabricated":false,"actualInstalled":false,
            "hypotheticalConsumptionOnly":true,"queuedProducer":queued,"actualPersonalVadProducer":integrated,
            "actualNativeIdentityReceiver":integrated,"readerComputeMs":reader_compute,"rows":reports})).unwrap()).unwrap();
        if let Some(capture_thread)=capture_thread{capture_thread.join().unwrap();}
        if network_prefix{asr.cancel();}
        assert!(crate::speaker_verification::native_word_profile_is_current(phrase,&profile));
    }
}

struct SourceMeasurement {
    start_samples: u64,
    end_samples: u64,
    original: SessionSpeakerObservation,
    partitions: Vec<SessionSpeakerObservation>,
    complete_partition: bool,
    no_voice:bool,
}

pub(crate) struct TrackedSourceAudio {
    epoch: SourceEpoch,
    model: Session,
    measurements: Vec<SourceMeasurement>,
    source_through_samples: Option<u64>,
    segmentation_support: Vec<(u64,u64)>,
    overlap_support: Vec<(u64,u64)>,
}

impl TrackedSourceAudio {
    pub(crate) fn captured_analysis_ms(&self)->Option<u64>{self.source_through_samples.map(|end|(end+610)/16)}
    #[cfg(any(test, feature = "recorded-endpoint-probe"))]
    pub(crate) fn diagnostic(&self,tracker:&TrackedSourcePresence,raw:&LocalSpeakerTimeline,timeline:&LocalSpeakerTimeline)->serde_json::Value {
        let owners=|timeline:&LocalSpeakerTimeline|timeline.presence.iter().chain(&timeline.identity)
            .filter(|s|s.kind==EvidenceKind::Owner).map(|s|(s.start_ms,s.end_ms)).collect::<Vec<_>>();
        serde_json::json!({"referenceSources":tracker.diagnostic(),"rawOwnerRanges":owners(raw),"fusedOwnerRanges":owners(timeline),
            "lastMeasurement":self.measurements.last().map(|m|serde_json::json!({"rangeMs":[m.start_samples/16,(m.end_samples+15)/16],
                "originalContextMs":[(m.end_samples+610-38400)/16,(m.end_samples+610)/16],
                "originalClass":format!("{:?}",m.original.classification),
                "originalReferenceMatches":tracker.reference_matches(&m.original),
                "partitionClasses":m.partitions.iter().map(|p|format!("{:?}",p.classification)).collect::<Vec<_>>(),
                "partitionReferenceMatches":m.partitions.iter().map(|p|tracker.reference_matches(p)).collect::<Vec<_>>(),
                "completePartition":m.complete_partition,"noVoiceObserved":m.no_voice}))})
    }
    pub(crate) fn load(path: &Path, epoch: SourceEpoch) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        if format!("{:x}", Sha256::digest(&bytes)) != MODEL_SHA {
            return Err("source segmentation model hash mismatch".into());
        }
        let dll = crate::persistence::speaker_verification_root().map_err(|e| e.to_string())?.join("onnxruntime.dll");
        let _ = ort::init_from(dll).map_err(|e| e.to_string())?.with_name("listener-source-presence").commit();
        let model = Session::builder().map_err(|e| e.to_string())?.with_intra_threads(2).map_err(|e| e.to_string())?
            .with_inter_threads(1).map_err(|e| e.to_string())?
            // Concurrent source, personal-activity and bank sessions must not
            // spend their neighbours' inference budget spinning while idle.
            .with_config_entry("session.intra_op.allow_spinning","0").map_err(|e|e.to_string())?
            .with_config_entry("session.inter_op.allow_spinning","0").map_err(|e|e.to_string())?
            .commit_from_memory(&bytes).map_err(|e| e.to_string())?;
        Ok(Self { epoch, model, measurements: Vec::new(), source_through_samples: None,segmentation_support:Vec::new(),overlap_support:Vec::new() })
    }

    /// Candidate contract: complete origin-zero captured input, at most 60s.
    /// Unsupported ranges return no evidence rather than fabricated silence.
    pub(crate) fn analyze(&mut self, epoch: SourceEpoch, pcm: &[u8], profile: &NativeWordSpeakerProfile) -> Result<bool, String> {
        if self.epoch != epoch { return Err("source capture epoch changed".into()); }
        if pcm.len() % 2 != 0 || pcm.len() < 320_000 || pcm.len() > 60_000 * 32 { return Ok(false); }
        let captured = (pcm.len() / 2) as u64;
        let through = captured - 610;
        if self.source_through_samples.is_some_and(|old| through <= old) { return Ok(false); }
        let origin = captured - 160_000;
        let clip = &pcm[pcm.len()-320_000..];
        let values: Vec<f32> = clip.chunks_exact(2).map(|b| i16::from_le_bytes([b[0],b[1]]) as f32 / 32768.0).collect();
        let input = TensorRef::from_array_view(([1usize,1usize,160_000usize], values.as_slice())).map_err(|e| e.to_string())?;
        #[cfg(test)] let segmentation_began=Instant::now();
        let output = self.model.run(ort::inputs!["x" => input]).map_err(|e| e.to_string())?;
        let (shape, logits) = output["y"].try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
        if shape.as_ref() != [1i64,589,7] || logits.iter().any(|f| !f.is_finite()) {
            return Err("invalid segmentation support contract".into());
        }
        let cells: Vec<_> = logits.chunks_exact(7).map(|row| CLASSES[row.iter().enumerate()
            .max_by(|a,b| a.1.total_cmp(b.1)).unwrap().0]).collect();
        self.segmentation_support.push((origin+360,through));
        for (i,cell) in cells.iter().enumerate().filter(|(_,cell)|cell.iter().filter(|v|**v).count()>1) {
            let center=origin+495+i as u64*270;self.overlap_support.push((center-135,center+135));
        }
        let start = self.source_through_samples.unwrap_or(through-2560);
        if start < origin+360 { return Err("source support has an unobserved gap".into()); }
        let mut active = [false;3];
        let mut complete = true;
        for (i, cell) in cells.iter().enumerate() {
            let center = origin+495+i as u64*270;
            if center+135 > start && center-135 < through {
                if cell.iter().filter(|v| **v).count() > 1 { complete = false; }
                for slot in 0..3 { active[slot] |= cell[slot]; }
            }
        }
        let mut partitions = Vec::new();
        #[cfg(test)] let segmentation_ms=segmentation_began.elapsed().as_millis();
        #[cfg(test)] let encoding_began=Instant::now();
        for slot in (0..3).filter(|slot| active[*slot]) {
            let mut selected = Vec::new();
            for (i, cell) in cells.iter().enumerate() {
                if cell[slot] && cell.iter().filter(|v| **v).count() == 1 {
                    let center = 495+i*270;
                    selected.extend_from_slice(&clip[(center-135)*2..(center+135)*2]);
                }
            }
            if selected.len() > 76_800 { selected.drain(..selected.len()-76_800); }
            if selected.len() < 800 { complete = false; continue; }
            partitions.push(profile.observe(&selected)?);
        }
        let original = profile.observe(&pcm[pcm.len()-76_800..])?;
        #[cfg(test)] eprintln!("[queued-source-stages] captured_ms={} segmentation_ms={segmentation_ms} encoding_ms={}",pcm.len()/32,encoding_began.elapsed().as_millis());
        self.measurements.push(SourceMeasurement { start_samples:start,end_samples:through,
            original,partitions,complete_partition:complete,no_voice:!active.iter().any(|v|*v) });
        self.source_through_samples = Some(through);
        Ok(true)
    }

    /// Resolve previously measured Unknown using newly qualified references,
    /// while preserving every raw positive and current native owner source.
    pub(crate) fn evidence(&self, epoch: SourceEpoch, tracker: &TrackedSourcePresence,
        profile: &NativeWordSpeakerProfile, raw: &LocalSpeakerTimeline, now: Instant)
        -> Option<(PresenceObservation, LocalSpeakerTimeline)> {
        if self.epoch != epoch { return None; }
        let through = self.source_through_samples?;
        let certificates=tracker.foreign_certificates();
        let mut qualified=Vec::new();
        for (a,b) in &certificates {
            let start=a*16;let end=b*16;
            if self.segmentation_support.iter().any(|(left,right)|*left<=start && end<=*right)
                && !self.overlap_support.iter().any(|(left,right)|*left<end && start<*right)
                && !tracker.known_owner_overlaps(*a,*b)
                && !tracker.unresolved_native_overlaps(*a,*b) {qualified.push((start,end));}
        }
        for measured in &self.measurements {
            if !measured.complete_partition || !tracker.partitions_support_foreign(&measured.partitions) {continue;}
            if tracker.source_negatives_corroborate_track(&measured.original,&measured.partitions)
                && !tracker.known_owner_overlaps(measured.start_samples/16,(measured.end_samples+15)/16)
                && !tracker.unresolved_native_overlaps(measured.start_samples/16,(measured.end_samples+15)/16) {
                qualified.push((measured.start_samples,measured.end_samples));
            }
            for (a,b) in &certificates {
                let start=measured.start_samples.max(a*16);let end=measured.end_samples.min(b*16);
                if start<end && !tracker.known_owner_overlaps(start/16,(end+15)/16)
                    && !tracker.unresolved_native_overlaps(start/16,(end+15)/16) {qualified.push((start,end));}
            }
        }
        qualified.sort_unstable();
        let mut merged:Vec<(u64,u64)>=Vec::new();
        for (start,end) in qualified {
            if let Some(last)=merged.last_mut().filter(|last|start<=last.1){last.1=last.1.max(end);}
            else{merged.push((start,end));}
        }
        let covered=|start:u64,end:u64|merged.iter().any(|(a,b)|*a<=start && end<=*b);
        let mut refined_raw=raw.clone();
        refined_raw.presence=raw.presence.iter().flat_map(|span| {
            let mut pieces=vec![(span.start_ms*16,span.end_ms*16)];
            if span.kind==EvidenceKind::Owner {
                for (a,b) in &merged {
                    pieces=pieces.into_iter().flat_map(|(start,end)| {
                        if *a>=end || *b<=start{return vec![(start,end)];}
                        let mut keep=Vec::new();if start<*a{keep.push((start,*a));}if *b<end{keep.push((*b,end));}keep
                    }).collect();
                }
            }
            pieces.into_iter().filter(|(a,b)|a<b).map(|(a,b)|SpeakerSpan{start_ms:a/16,end_ms:(b+15)/16,kind:span.kind}).collect::<Vec<_>>()
        }).collect();
        let mut spans: Vec<(u64,u64,PresenceState)> = Vec::new();
        for measured in &self.measurements {
            let start = measured.start_samples; let end = measured.end_samples;
            let decision = tracker.decide(epoch,&measured.original,&measured.partitions,profile);
            let raw_owner = refined_raw.presence.iter().any(|s| s.kind==EvidenceKind::Owner && s.start_ms*16<end && start<s.end_ms*16);
            let native_owner = tracker.known_owner_overlaps(start/16,(end+15)/16);
            // A delayed measurement may span foreign words followed by a
            // genuine owner return. Its coarse positive protects the latter;
            // it cannot overwrite independently qualified foreign coordinates
            // in the former. Split at proof boundaries before assigning roles.
            let mut cuts=vec![start,end];
            for (a,b) in &merged {
                if *a<end && start<*b {cuts.push(start.max(*a));cuts.push(end.min(*b));}
            }
            for (a,b) in tracker.unresolved_native_ranges() {
                let (a,b)=(a*16,b*16);
                if a<end && start<b {cuts.push(start.max(a));cuts.push(end.min(b));}
            }
            cuts.sort_unstable();cuts.dedup();
            for range in cuts.windows(2) {
                let (start,end)=(range[0],range[1]);
                let precise_native_owner=tracker.known_owner_overlaps(start/16,(end+15)/16);
                let unresolved_native=tracker.unresolved_native_overlaps(start/16,(end+15)/16);
                let state = if !precise_native_owner && !unresolved_native && covered(start,end) {PresenceState::ConfirmedOther}
                    else if raw_owner || native_owner || decision==TrackedSourceDecision::Owner {PresenceState::Owner}
                    else if !unresolved_native && measured.complete_partition && measured.no_voice {PresenceState::ObservedSilence}
                    else if !unresolved_native && measured.complete_partition && decision==TrackedSourceDecision::CorroboratedForeign {PresenceState::ConfirmedOther}
                    else {PresenceState::Unknown};
                if let Some(last)=spans.last_mut().filter(|last|last.1==start && last.2==state){last.1=end;}
                else{spans.push((start,end,state));}
            }
        }
        let mut timeline = refined_raw;
        // Unscored right-edge Unknown is outside this observation's support.
        // Raw positives there still veto; freshness is checked by the endpoint.
        for span in &mut timeline.presence {
            if span.kind != EvidenceKind::Owner { span.end_ms=span.end_ms.min(through/16); }
        }
        timeline.presence.retain(|s| s.start_ms<s.end_ms);
        // Other and silence both prove owner absence, while only Other may
        // carry word identity. Join their contiguous sample support before
        // inward rounding, so a fractional transition cannot invent a 1ms
        // unresolved-speech island. Missing support/positives remain gaps.
        let mut absence:Vec<(u64,u64)>=Vec::new();
        for (a,b,_) in spans.iter().filter(|(_,_,state)|matches!(state,PresenceState::ConfirmedOther|PresenceState::ObservedSilence)) {
            if let Some(last)=absence.last_mut().filter(|last|last.1==*a) {last.1=*b;}
            else {absence.push((*a,*b));}
        }
        for (a,b) in &absence {
            let (a,b)=((a+15)/16,b/16);
            timeline.presence=timeline.presence.into_iter().flat_map(|span| {
                if span.kind!=EvidenceKind::Unknown || a>=span.end_ms || b<=span.start_ms {return vec![span];}
                let mut keep=Vec::new();if span.start_ms<a {keep.push(SpeakerSpan{end_ms:a,..span});}
                if b<span.end_ms {keep.push(SpeakerSpan{start_ms:b,..span});}keep
            }).collect();
            timeline.note_presence(SpeakerSpan{start_ms:a,end_ms:b,kind:EvidenceKind::Other});
        }
        for (start,end) in &merged {
            timeline.note_identity(SpeakerSpan{start_ms:(start+15)/16,end_ms:end/16,kind:EvidenceKind::Other});
            timeline.note_presence(SpeakerSpan{start_ms:(start+15)/16,end_ms:end/16,kind:EvidenceKind::Other});
        }
        for (start,end,state) in &spans {
            let kind = match state { PresenceState::Owner=>EvidenceKind::Owner,PresenceState::ConfirmedOther=>EvidenceKind::Other,PresenceState::Unknown=>EvidenceKind::Unknown,PresenceState::ObservedSilence=>EvidenceKind::Other };
            // Merge adjacent sample support before outward-positive/inward-
            // negative rounding, so no intermediate fractional gap is filled.
            let (start_ms,end_ms) = if matches!(state,PresenceState::ConfirmedOther|PresenceState::ObservedSilence) { ((start+15)/16,end/16) }
                else { (start/16,(end+15)/16) };
            if *state!=PresenceState::ObservedSilence {timeline.note_identity(SpeakerSpan{start_ms,end_ms,kind});}
            timeline.note_presence(SpeakerSpan{start_ms,end_ms,kind});
        }
        let owner_end = timeline.presence.iter().chain(&timeline.identity).filter(|s| s.kind==EvidenceKind::Owner).map(|s|s.end_ms).max();
        let unknown_end = timeline.unresolved_activity_end_ms_after(owner_end.unwrap_or_default());
        Some((PresenceObservation { analyzed_end_ms:through/16,last_positive_end_ms:owner_end,
            last_unresolved_end_ms:unknown_end,source_history_complete:true,state:spans.last()?.2,observed_at:now },timeline))
    }
}
