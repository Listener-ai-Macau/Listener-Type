//! Exact native source identity for ordinary ASR. This never owns STOP,
//! wake admission, protected enrollment, or an editor transaction.

use super::volcengine_speaker_ledger::{aligned_rows, EvidenceKind, LocalSpeakerTimeline, SourceRow};
use crate::speaker_verification::{self, SessionSpeakerClassification, SessionSpeakerObservation};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default)]
pub(crate) struct NativeWordIdentity {
    pub qualified_reference: bool,
    pub rows: Vec<(u64, u64, EvidenceKind)>,
    pub raw_activity: LocalSpeakerTimeline,
    binding: Option<(String, [u8; 32])>,
}

impl NativeWordIdentity {
    #[cfg(test)]
    fn recorded_projection_for_test(phrase: &str, projection: &Value, raw_activity: LocalSpeakerTimeline) -> Self {
        let kind=|value:&Value| match value.as_str().unwrap() {"Owner"=>EvidenceKind::Owner,"Other"=>EvidenceKind::Other,_=>EvidenceKind::Unknown};
        let qualified_reference=projection["qualified_reference"].as_bool().unwrap();
        let rows=projection["source_rows"].as_array().unwrap().iter().map(|row|
            (row[0].as_u64().unwrap(),row[1].as_u64().unwrap(),kind(&row[2]))).collect();
        // Replay the captured decision, not a new model inference. Loading the
        // unchanged protected bank supplies lifecycle binding only; no natural
        // reference is fabricated or installed into the production profile.
        let binding=qualified_reference.then(|| (phrase.to_string(),
            speaker_verification::native_word_profile_for_phrase(phrase).unwrap().unwrap().binding_key()));
        Self {qualified_reference,rows,raw_activity,binding}
    }
    pub(crate) fn invalidate_if_profile_changed(&mut self) {
        if !self.binding.as_ref().is_some_and(|(phrase,key)|
            speaker_verification::native_word_binding_is_current(phrase,*key)) {
            self.qualified_reference = false;
            self.rows.clear();
        }
    }
    pub(super) fn kind(&self, row: &SourceRow) -> EvidenceKind {
        if !row.definite || row.speaker.is_none() { return EvidenceKind::Unknown; }
        self.rows.iter().find(|(start, end, _)| *start == row.start && *end == row.end)
            .map_or(EvidenceKind::Unknown, |(_, _, kind)| *kind)
    }

    pub(super) fn detector_excludes_owner(&self, start: u64, end: u64) -> bool {
        detector_observed_without_owner(&self.raw_activity, start, end)
    }
}

/// Complete raw detector observation is distinct from a rolling identity
/// rescue. Missing coverage and any actual detector positive veto absence.
pub(crate) fn detector_observed_without_owner(raw: &LocalSpeakerTimeline, start: u64, end: u64) -> bool {
        if start >= end || raw.presence.iter().any(|span|
            span.kind == EvidenceKind::Owner && overlap(start, end, span.start_ms, span.end_ms)) {
            return false;
        }
        let mut spans: Vec<_> = raw.presence.iter().filter(|span|
            span.kind == EvidenceKind::Unknown && overlap(start, end, span.start_ms, span.end_ms))
            .map(|span| (span.start_ms, span.end_ms)).collect();
        spans.sort_unstable();
        let mut frontier = start;
        for (begin, finish) in spans {
            if begin > frontier { return false; }
            frontier = frontier.max(finish);
            if frontier >= end { return true; }
        }
        false
}

fn overlap(a: u64, b: u64, c: u64, d: u64) -> bool { a < d && c < b }

/// Native separation must account for provisional rows too. A definite row
/// cannot borrow clean identity when another anonymous source overlaps it.
fn exclusive(row: &SourceRow, rows: &[SourceRow]) -> bool {
    row.definite && row.speaker.is_some() && rows.iter().all(|other|
        !overlap(row.start, row.end, other.start, other.end) || row.speaker == other.speaker)
}

fn exact_pcm(pcm: &[u8], start: u64, end: u64) -> Option<&[u8]> {
    if start >= end { return None; }
    let begin = usize::try_from(start).ok()?.checked_mul(32)?;
    let finish = usize::try_from(end).ok()?.checked_mul(32)?;
    pcm.get(begin..finish)
}

fn qualified_reference_ranges(rows: &[SourceRow], local: &LocalSpeakerTimeline,
    raw: &LocalSpeakerTimeline) -> Vec<(u64, u64)> {
    local.identity.iter().filter(|span| span.kind == EvidenceKind::Owner).filter_map(|span| {
        let touching: Vec<_> = rows.iter().filter(|row| overlap(span.start_ms, span.end_ms, row.start, row.end)).collect();
        let first = touching.first()?;
        if first.speaker.is_none() || touching.iter().any(|row| !row.definite || row.speaker != first.speaker) {
            return None;
        }
        let positive = touching.iter().any(|row| row.word_ranges().any(|(start, end)|
            raw.presence.iter().any(|activity| activity.kind == EvidenceKind::Owner
                && span.start_ms.max(start).max(activity.start_ms) < span.end_ms.min(end).min(activity.end_ms))));
        positive.then_some((span.start_ms, span.end_ms))
    }).collect()
}

#[derive(Default)]
pub(crate) struct NativeSourceIdentity {
    // Audio coordinates belong to this ASR capture. Cached embeddings cannot
    // survive reset; spelling/ID revisions always revalidate current rows.
    observations: BTreeMap<(u64, u64), SessionSpeakerObservation>,
}

impl NativeSourceIdentity {
    pub(crate) fn reset(&mut self) { self.observations.clear(); }

    pub(crate) fn analyze(&mut self, phrase: &str, result: &Value, pcm: &[u8],
        local: &LocalSpeakerTimeline, raw: LocalSpeakerTimeline) -> NativeWordIdentity {
        let mut output = NativeWordIdentity { raw_activity: raw, ..Default::default() };
        let Some(rows) = aligned_rows(result) else { return output; };
        let Ok(Some(mut profile)) = speaker_verification::native_word_profile_for_phrase(phrase) else { return output; };
        if !profile.has_qualified_reference {
            // A short first positive window may fail reference quality while a
            // later independent Owner window is usable. Rejections are source
            // observations, not a permanent claim on the acquisition slot.
            // Reuse measured failures and admit at most one new inference per
            // provider packet, preserving the existing receive-path work bound.
            let mut inferred_reference = false;
            for (start, end) in qualified_reference_ranges(&rows, local, &output.raw_activity) {
                if let Some(source) = exact_pcm(pcm, start, end) {
                    let key=(start,end);
                    if !self.observations.contains_key(&key) {
                        if inferred_reference { continue; }
                        inferred_reference = true;
                        if let Ok(observation)=profile.observe(source) { self.observations.insert(key,observation); }
                    }
                    if let Some(observation)=self.observations.get(&key) {
                        if speaker_verification::acquire_native_word_reference(phrase, &profile, observation) {
                            if let Ok(Some(updated)) = speaker_verification::native_word_profile_for_phrase(phrase) {
                                profile = updated;
                            }
                            break;
                        }
                    }
                }
            }
        }
        // No acquired natural reference: preserve the established product
        // behavior, including its conservative treatment of weak own speech.
        if !profile.has_qualified_reference { return output; }
        for row in rows.iter().filter(|row| exclusive(row, &rows)) {
            let key = (row.start, row.end);
            if !self.observations.contains_key(&key) {
                let Some(source) = exact_pcm(pcm, row.start, row.end) else { continue; };
                let Ok(observation) = profile.observe(source) else { continue; };
                self.observations.insert(key, observation);
            }
            let observation = &self.observations[&key];
            let kind = match profile.classify(observation) {
                SessionSpeakerClassification::Target { .. } => EvidenceKind::Owner,
                SessionSpeakerClassification::NonTarget { .. } if observation.signal_quality_sufficient => EvidenceKind::Other,
                _ => EvidenceKind::Unknown,
            };
            output.rows.push((row.start, row.end, kind));
        }
        if !speaker_verification::native_word_profile_is_current(phrase, &profile) {
            output.rows.clear();
            return output;
        }
        output.qualified_reference = true;
        output.binding = Some((phrase.to_string(), profile.binding_key()));
        output
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;
    use super::super::volcengine_speaker_ledger::SpeakerSpan;
    fn packet(rows: Vec<Value>) -> Value {
        json!({"text": rows.iter().map(|row|row["text"].as_str().unwrap()).collect::<String>(), "utterances":rows})
    }
    fn row(text: &str, id: &str, start: u64, end: u64, definite: bool) -> Value {
        json!({"text":text,"definite":definite,"additions":{"speaker_id":id},
            "words":[{"text":text,"start_time":start,"end_time":end}]})
    }

    #[test]
    #[ignore = "explicit private saved decision replay; no model, cloud, capture or editor activity"]
    fn recorded_native_turn_boundary_veto_does_not_restore_other_tail_in_callbacks() {
        use super::super::volcengine::{VolcengineCredentials,VolcengineStreamingASR};
        use super::super::volcengine_speaker_ledger::SpeakerSpan;
        let path=std::env::var("LISTENER_NATIVE_VETO_TRACE").unwrap();
        let archived:Value=serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let frames:Vec<_>=archived["trace"]["entries"].as_array().unwrap().iter()
            .filter(|entry|entry["stage"]=="normalized_result").collect();
        let original_entries=archived["trace"]["entries"].as_array().unwrap();
        let mut actual=VolcengineStreamingASR::new(VolcengineCredentials {
            app_id:"offline".into(),access_token:"offline".into(),resource_id:"offline".into()},Vec::new())
            .with_owner_activity_only(true);
        actual.enable_diagnostic_trace();
        let mut local=LocalSpeakerTimeline::default();
        let mut raw=LocalSpeakerTimeline::default();
        let mut comparisons=Vec::new();
        for entry in &frames {
            let projection=&entry["speaker_projection"];
            for (field,identity) in [("local_identity_newest_first",true),("local_presence_newest_first",false)] {
                for value in projection[field].as_array().unwrap().iter().rev() {
                    let kind=match value[2].as_str().unwrap() {"Owner"=>EvidenceKind::Owner,"Other"=>EvidenceKind::Other,_=>EvidenceKind::Unknown};
                    let span=SpeakerSpan {start_ms:value[0].as_u64().unwrap(),end_ms:value[1].as_u64().unwrap(),kind};
                    let target=if identity {&mut local.identity}else{&mut local.presence};
                    if !target.iter().any(|old|old.start_ms==span.start_ms && old.end_ms==span.end_ms && old.kind==span.kind) {target.push(span);}
                    if !identity {
                        // A fused Other necessarily had a negative detector
                        // score. Keep every captured Owner (including rescues)
                        // as an extra veto. This is a conservative reconstruction,
                        // not the unavailable original raw detector snapshot.
                        let raw_kind=if kind==EvidenceKind::Owner {EvidenceKind::Owner}else{EvidenceKind::Unknown};
                        let raw_span=SpeakerSpan {kind:raw_kind,..span};
                        if !raw.presence.iter().any(|old|old.start_ms==raw_span.start_ms && old.end_ms==raw_span.end_ms && old.kind==raw_span.kind) {
                            raw.presence.push(raw_span);
                        }
                    }
                }
            }
            actual.install_native_word_identity_for_test(NativeWordIdentity::recorded_projection_for_test(
                "开始录音",&projection["native_word_identity"],raw.clone()));
            actual.apply_saved_native_result_with_local_timeline_for_test(&entry["provider_result"],entry["final_frame"]==true,&local);
        }
        let trace=actual.take_diagnostic_trace();
        let mut callback_checks=0;
        let mut callback_violations=Vec::new();
        for entry in frames.iter().filter(|entry|entry["frame"].as_u64().unwrap()>=35) {
            let frame=entry["frame"].as_u64().unwrap();
            let old=original_entries.iter().find(|e|e["frame"]==frame && e["stage"]=="speaker_filtered_result").unwrap();
            let filtered=trace.entries.iter().find(|e|e.frame as u64==frame && e.stage=="speaker_filtered_result").unwrap();
            let key=crate::transcript_boundary::content_key(&filtered.text);
            let owner_rows:Vec<_>=entry["provider_result"]["utterances"].as_array().unwrap().iter()
                .filter(|row|row["additions"]["speaker_id"]=="1").collect();
            let missing:Vec<_>=owner_rows.iter().filter_map(|row| {
                let text=row["text"].as_str().unwrap();
                (!key.contains(&crate::transcript_boundary::content_key(text))).then_some(text)
            }).collect();
            assert!(missing.is_empty(),"existing owner source was changed at frame {frame}: {missing:?}");
            assert!(key.contains("至"),"the boundary word containing positive evidence was removed");
            assert!(!key.contains("最高领奖台"),"complete confirmed Other tail was protected by the boundary word");
            for callback in trace.entries.iter().filter(|e|e.frame as u64==frame && e.stage.contains("callback")) {
                callback_checks+=1;
                if crate::transcript_boundary::content_key(&callback.text).contains("最高领奖台") {
                    callback_violations.push(serde_json::json!({"frame":frame,"stage":callback.stage,"text":callback.text}));
                }
            }
            comparisons.push(serde_json::json!({"frame":frame,"baseline":old["text"],"candidate":filtered.text,
                "missing_owner_rows":missing}));
        }
        let report=serde_json::json!({"schema":"listener.native-source-veto-scope.v1","session":archived["sessionId"],
            "frames":frames.len(),"comparisons":comparisons,"callback_checks":callback_checks,"callback_violations":callback_violations,
            "cloud_calls":0,"model_inferences":0,"editor_writes":0,"protected_profile_writes":0,"trace":trace,
            "scope":"Original provider packets and captured identity/activity decisions through production projection and callbacks. Raw negatives reconstructed conservatively from fused observations, with all captured positives retained. The archived session was cancelled; no final or editor receipt invented, no new live/model-accuracy claim."});
        std::fs::write(std::env::var("LISTENER_NATIVE_VETO_OUTPUT").unwrap(),serde_json::to_vec_pretty(&report).unwrap()).unwrap();
        assert!(callback_checks>0 && callback_violations.is_empty(),"excluded source was restored by a production callback");
    }

    #[test]
    fn native_reference_requires_exclusive_definite_source_and_real_activity() {
        let local = LocalSpeakerTimeline { identity:vec![SpeakerSpan {start_ms:0,end_ms:1400,kind:EvidenceKind::Owner}], ..Default::default() };
        let mut raw = LocalSpeakerTimeline::default();
        let clean=aligned_rows(&packet(vec![row("Owner", "1", 100, 1800, true)])).unwrap();
        assert_eq!(qualified_reference_ranges(&clean,&local,&raw),Vec::<(u64,u64)>::new(),"rolling identity cannot stand in for the detector");
        raw.note_presence(SpeakerSpan {start_ms:200,end_ms:400,kind:EvidenceKind::Owner});
        assert_eq!(qualified_reference_ranges(&clean,&local,&raw),vec![(0,1400)]);
        for other_definite in [true,false] {
            let mixed=aligned_rows(&packet(vec![row("Owner","1",100,1800,true),row("Other","2",900,1900,other_definite)])).unwrap();
            assert_eq!(qualified_reference_ranges(&mixed,&local,&raw),Vec::<(u64,u64)>::new());
            assert!(!exclusive(&mixed[0],&mixed));
        }
        assert!(exact_pcm(&[0;32],0,2).is_none(),"no padding or clamping of unavailable source");
    }

    #[test]
    #[ignore = "requires the original failed source projection and protected registration; no device/cloud/editor"]
    fn saved_short_reference_rejection_does_not_starve_later_owner_source() {
        use std::time::Instant;
        let root=std::path::PathBuf::from(std::env::var_os("LISTENER_REFERENCE_STARVATION").unwrap());
        let fixture:Value=serde_json::from_slice(&std::fs::read(root.join("fixture.json")).unwrap()).unwrap();
        let phrase=fixture["phrase"].as_str().unwrap();
        let pcm=std::fs::read(fixture["pcm"].as_str().unwrap()).unwrap();
        let trace:Value=serde_json::from_slice(&std::fs::read(fixture["trace"].as_str().unwrap()).unwrap()).unwrap();
        let restore=|key:&str| {
            let convert=|row:&Value| SpeakerSpan {start_ms:row["start_ms"].as_u64().unwrap(),end_ms:row["end_ms"].as_u64().unwrap(),
                kind:match row["kind"].as_str().unwrap() {"Owner"=>EvidenceKind::Owner,"Other"=>EvidenceKind::Other,_=>EvidenceKind::Unknown}};
            LocalSpeakerTimeline {identity:fixture[key]["identity"].as_array().unwrap().iter().map(convert).collect(),
                presence:fixture[key]["presence"].as_array().unwrap().iter().map(convert).collect()}
        };
        let local=restore("local");let raw=restore("raw");
        use super::super::volcengine::{VolcengineCredentials, VolcengineStreamingASR};
        let offline=|| {
            let mut asr=VolcengineStreamingASR::new(VolcengineCredentials {
                app_id:"offline".into(),access_token:"offline".into(),resource_id:"offline".into(),
            },Vec::new()).with_owner_activity_only(true);
            asr.note_accepted_wake_phrase(phrase);asr.enable_diagnostic_trace();asr
        };
        let mut actual=offline();let mut baseline=offline();let mut body_comparisons=Vec::new();
        let mut analyzer=NativeSourceIdentity::default();let mut outcomes=Vec::new();
        let began=Instant::now();let mut first_qualified=None;
        for frame in trace["trace"]["entries"].as_array().unwrap().iter().filter(|row|
            row["stage"]=="normalized_result" && row["provider_result"].is_object()) {
            if frame["provider_result"]["utterances"].as_array().is_some_and(|rows| rows.iter().any(|row|
                row["end_time"].as_u64().is_some_and(|end| end>fixture["max_source_ms"].as_u64().unwrap()))) {continue;}
            let before=analyzer.observations.len();let tick=Instant::now();
            let result=analyzer.analyze(phrase,&frame["provider_result"],&pcm,&local,raw.clone());
            if result.qualified_reference && first_qualified.is_none() {first_qualified=Some(frame["frame"].clone());}
            actual.install_native_word_identity_for_test(result.clone());
            // Before-fix saved qualification was false for every one of these
            // packets. Reuse that exact old decision, with restored raw source,
            // rather than changing the cloud response or inventing an EOF.
            baseline.install_native_word_identity_for_test(NativeWordIdentity {raw_activity:raw.clone(),..Default::default()});
            let new=actual.apply_saved_native_result_with_local_timeline_for_test(&frame["provider_result"],false,&local);
            let old=baseline.apply_saved_native_result_with_local_timeline_for_test(&frame["provider_result"],false,&local);
            let new_body=new.map(|row|row.text);let old_body=old.map(|row|row.text);
            assert_eq!(new_body,old_body,"this failed capture's owner/provisional text must not be lost when reference qualification becomes active");
            body_comparisons.push(serde_json::json!({"frame":frame["frame"],"old":old_body,"new":new_body}));
            let reference_cached=analyzer.observations.get(&(0,2400)).map(|row|
                serde_json::json!({"speechMs":row.real_speech_ms,"quality":row.signal_quality_sufficient}));
            outcomes.push(serde_json::json!({"frame":frame["frame"],"qualified":result.qualified_reference,
                "newObservations":analyzer.observations.len()-before,"elapsedMs":tick.elapsed().as_millis(),
                "firstCandidate":reference_cached,"rows":result.rows.iter().map(|(a,b,k)|
                    serde_json::json!([a,b,format!("{:?}",k)])).collect::<Vec<_>>()}));
        }
        let first=analyzer.observations.get(&(0,2400)).expect("the rejected real first source is retained");
        assert_eq!(first.real_speech_ms,600);assert!(!first.signal_quality_sufficient);
        let later=analyzer.observations.get(&(2400,4800)).expect("later independent source was attempted");
        assert_eq!(later.real_speech_ms,2300);assert!(later.signal_quality_sufficient);
        assert!(first_qualified.is_some(),"a rejected short first window must not hold the only reference slot forever");
        assert!(analyzer.observations.len()>1);
        std::fs::write(root.join("after-reference-result.json"),serde_json::to_vec_pretty(&serde_json::json!({
            "firstQualifiedFrame":first_qualified,"frames":outcomes,"bodyComparisons":body_comparisons,
            "projectionTrace":actual.take_diagnostic_trace(),"elapsedSeconds":began.elapsed().as_secs_f64(),
            "enrollmentWrites":0,"cloudCalls":0,"actualEditor":false,"actualStop":false,
            "scope":"Original normalized packets and actual saved activity source; existing protected identity inference. Reference acquisition only; no forced STOP or end-to-end acceptance."})).unwrap()).unwrap();
    }

    #[test]
    fn raw_detector_gaps_and_positives_veto_native_exclusion() {
        let mut output=NativeWordIdentity::default();
        output.raw_activity.note_presence(SpeakerSpan {start_ms:0,end_ms:1000,kind:EvidenceKind::Unknown});
        assert!(output.detector_excludes_owner(100,900));
        assert!(!output.detector_excludes_owner(100,1100));
        output.raw_activity.note_presence(SpeakerSpan {start_ms:500,end_ms:660,kind:EvidenceKind::Owner});
        assert!(!output.detector_excludes_owner(100,900));
    }

    #[test]
    #[ignore = "requires the archived source PCM/native responses and current protected registration; no cloud or editor writes"]
    pub(crate) fn recorded_native_source_projection_keeps_owner_and_excludes_foreign_without_endpoint_changes() {
        use super::super::volcengine::{VolcengineCredentials, VolcengineStreamingASR};
        use crate::personal_vad::PersonalVadStream;
        use std::{time::{Duration, Instant}, sync::Arc};
        struct ReplayLogger;
        impl log::Log for ReplayLogger {
            fn enabled(&self, metadata:&log::Metadata<'_>)->bool { metadata.level()<=log::Level::Warn }
            fn log(&self, record:&log::Record<'_>) { if self.enabled(record.metadata()) { eprintln!("{} {}",record.level(),record.args()); } }
            fn flush(&self) {}
        }
        static LOGGER:ReplayLogger=ReplayLogger;
        let _=log::set_logger(&LOGGER);
        log::set_max_level(log::LevelFilter::Warn);
        let protocol:Value=serde_json::from_slice(&std::fs::read(std::env::var("LISTENER_NATIVE_SOURCE_REPAIR_INPUT").unwrap()).unwrap()).unwrap();
        let phrase=protocol["phrase"].as_str().unwrap();
        let mut reports=Vec::new();
        for case in protocol["cases"].as_array().unwrap() {
            eprintln!("[saved-source] starting {}",case["session"]);
            let pcm=std::fs::read(case["pcm"].as_str().unwrap()).unwrap();
            // Real automatic wake has already initialized the independent
            // enrolled scorer before opening body activity. Recreate that
            // runtime prerequisite; this observation grants no permission,
            // cannot acquire an exemplar and is not a fabricated Owner span.
            if reports.is_empty() {
                let bootstrap=speaker_verification::native_word_profile_for_phrase(phrase).unwrap().unwrap();
                let _=bootstrap.observe(exact_pcm(&pcm,0,1440).unwrap()).unwrap();
                eprintln!("[saved-source] independent scorer runtime ready");
            }
            let reference=speaker_verification::personal_activity_reference_for_phrase(phrase).unwrap().unwrap();
            let stream=PersonalVadStream::start(Vec::new(),Some(reference),
                speaker_verification::endpoint_identity_verifier_for_phrase(phrase).unwrap());
            // Exercise the real bounded worker, including its existing batch
            // semantics. Source clocks remain those of the archived capture.
            stream.submit(&pcm, (pcm.len()/2) as u64);
            let expected_end=(pcm.len()/5120*160) as u64;
            let began=Instant::now();
            while stream.raw_activity_timeline().presence.iter().map(|span|span.end_ms).max().unwrap_or(0)<expected_end {
                assert!(!stream.analysis_unavailable_for_test(),"real activity worker reported unavailable; do not fabricate negative source coverage");
                assert!(began.elapsed()<Duration::from_secs(40),"real activity worker did not finish source: expected={} observed={:?} snapshot={:?}",
                    expected_end,stream.raw_activity_timeline().presence.iter().map(|span|span.end_ms).max(),stream.snapshot());
                std::thread::sleep(Duration::from_millis(20));
            }
            let raw=stream.raw_activity_timeline();
            let trace:Value=serde_json::from_slice(&std::fs::read(case["trace"].as_str().unwrap()).unwrap()).unwrap();
            let frames:Vec<_>=trace["trace"]["entries"].as_array().unwrap().iter()
                .filter(|entry|entry["stage"]=="normalized_result" && entry["provider_result"].is_object()).collect();
            assert!(!frames.is_empty());
            let credentials=VolcengineCredentials {app_id:"offline".into(),access_token:"offline".into(),resource_id:"offline".into()};
            let mut actual=VolcengineStreamingASR::new(credentials.clone(),Vec::new()).with_owner_activity_only(true);
            actual.enable_diagnostic_trace();
            let actual=Arc::new(actual);
            let baseline=VolcengineStreamingASR::new(credentials,Vec::new()).with_owner_activity_only(true);
            actual.note_accepted_wake_phrase(phrase);
            baseline.note_accepted_wake_phrase(phrase);
            let mut analyzer=NativeSourceIdentity::default();
            let mut local=LocalSpeakerTimeline::default();
            let mut native_rows=Vec::new();
            let mut final_actual=None;
            let mut final_baseline=None;
            let mut inference_ms=Vec::new();
            for (index, entry) in frames.iter().enumerate() {
                let view=&entry["speaker_projection"];
                for (field,identity) in [("local_identity_newest_first",true),("local_presence_newest_first",false)] {
                    if let Some(spans)=view[field].as_array() {
                        for value in spans.iter().rev() {
                            let span=SpeakerSpan {start_ms:value[0].as_u64().unwrap(),end_ms:value[1].as_u64().unwrap(),
                                kind:match value[2].as_str().unwrap() {"Owner"=>EvidenceKind::Owner,"Other"=>EvidenceKind::Other,_=>EvidenceKind::Unknown}};
                            if identity {
                                if !local.identity.iter().any(|old|old.start_ms==span.start_ms && old.end_ms==span.end_ms && old.kind==span.kind) {local.note_identity(span);}
                            } else {local.note_presence(span);}
                        }
                    }
                }
                let result=&entry["provider_result"];
                let began=Instant::now();
                let native=analyzer.analyze(phrase,result,&pcm,&local,raw.clone());
                inference_ms.push(began.elapsed().as_millis() as u64);
                if index+1==frames.len() {
                    native_rows=aligned_rows(result).unwrap().iter().map(|row|
                        serde_json::json!({"text":row.text,"speaker_id":row.speaker,"range_ms":[row.start,row.end],
                            "kind":format!("{:?}",native.kind(row)),
                            "raw_detector_allows_exclusion":native.detector_excludes_owner(row.start,row.end)})).collect();
                }
                actual.install_native_word_identity_for_test(native);
                let is_final=index+1==frames.len();
                final_actual=actual.apply_saved_native_result_with_local_timeline_for_test(result,is_final,&local).or(final_actual);
                final_baseline=baseline.apply_saved_native_result_with_local_timeline_for_test(result,is_final,&local).or(final_baseline);
            }
            let output=final_actual.unwrap().text;
            let original=final_baseline.unwrap().text;
            let key=crate::transcript_boundary::content_key(&output);
            let missing:Vec<_>=case["owner_rows"].as_array().unwrap().iter().filter_map(|text| {
                let text=text.as_str().unwrap();
                (!key.contains(&crate::transcript_boundary::content_key(text))).then_some(text)
            }).collect();
            let foreign_removed:Vec<_>=case["foreign_rows"].as_array().unwrap().iter().filter_map(|text| {
                let text=text.as_str().unwrap();
                (!key.contains(&crate::transcript_boundary::content_key(text))).then_some(text)
            }).collect();
            let trace=actual.take_diagnostic_trace();
            let mut callback_source_checks=0;
            let mut callback_source_violations=Vec::new();
            // Inspect the actual production callbacks, not just final text.
            // The excluded source rows come from this response's projection;
            // human labels are never used to grant model/word permissions.
            for packet in trace.entries.iter().filter(|entry|entry.stage=="normalized_result") {
                let Some(result)=packet.provider_result.as_ref() else {continue};
                if !packet.speaker_projection.as_ref().is_some_and(|view|
                    view["excluded_rows"].as_u64().unwrap_or(0)>0) {continue;}
                let Some(filtered)=trace.entries.iter().find(|entry|
                    entry.frame==packet.frame && entry.stage=="speaker_filtered_result") else {continue};
                let filtered_key=crate::transcript_boundary::content_key(&filtered.text);
                for row in aligned_rows(result).unwrap() {
                    let row_key=crate::transcript_boundary::content_key(&row.text);
                    if row_key.is_empty() || filtered_key.contains(&row_key) {continue;}
                    for callback in trace.entries.iter().filter(|entry|
                        entry.frame==packet.frame && entry.stage.contains("callback")) {
                        callback_source_checks+=1;
                        if crate::transcript_boundary::content_key(&callback.text).contains(&row_key) {
                            callback_source_violations.push(serde_json::json!({
                                "frame":packet.frame,"stage":callback.stage,"excluded_row":row.text,
                                "callback_text":callback.text}));
                        }
                    }
                }
            }
            reports.push(serde_json::json!({"session":case["session"],"native_frames":frames.len(),"baseline_final":original,
                "candidate_final":output,"missing_owner_rows":missing,"foreign_rows_removed":foreign_removed,
                "native_rows":native_rows,"identity_decision_ms":inference_ms,
                "callback_source_checks":callback_source_checks,"callback_source_violations":callback_source_violations,
                "activity_worker_ms":began.elapsed().as_millis(),"trace":trace}));
        }
        let report=serde_json::json!({"schema":"listener.native-source-identity-repair.v1","cases":reports,
            "cloud_calls":0,"editor_writes":0,"protected_template_writes":0,
            "scope":"Actual protected profile, source qualification, production activity worker, native packet projection and shared final arbitration. Accelerated saved replay; no real editor receipt, real-time endpoint acceptance or new device session."});
        std::fs::write(std::env::var("LISTENER_NATIVE_SOURCE_REPAIR_OUTPUT").unwrap(),serde_json::to_vec_pretty(&report).unwrap()).unwrap();
        assert!(report["cases"].as_array().unwrap().iter().all(|case|case["missing_owner_rows"].as_array().unwrap().is_empty()),"saved owner regression remains");
        assert!(report["cases"].as_array().unwrap().iter().any(|case|!case["foreign_rows_removed"].as_array().unwrap().is_empty()),"no foreign source was removed");
        assert!(report["cases"].as_array().unwrap().iter().any(|case|case["callback_source_checks"].as_u64().unwrap()>0),"saved responses did not exercise excluded-source callbacks");
        assert!(report["cases"].as_array().unwrap().iter().all(|case|case["callback_source_violations"].as_array().unwrap().is_empty()),"live callback restored an exact excluded source");
    }

    // Test-only old input-contract candidate combined with14's fresh recorded
    // source and15's current acquisition. It is not compiled into the product.
    mod refreshed_long_contract {
//! Exact native source identity for ordinary ASR. This never owns STOP,
//! wake admission, protected enrollment, or an editor transaction.

use crate::asr::volcengine_speaker_ledger::{aligned_rows, EvidenceKind, LocalSpeakerTimeline, SourceRow};
use crate::speaker_verification::{self, SessionSpeakerClassification, SessionSpeakerObservation};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default)]
pub(crate) struct NativeWordIdentity {
    pub qualified_reference: bool,
    pub rows: Vec<(u64, u64, EvidenceKind)>,
    pub raw_activity: LocalSpeakerTimeline,
    binding: Option<(String, [u8; 32])>,
}

impl NativeWordIdentity {
    #[cfg(test)]
    fn recorded_projection_for_test(phrase: &str, projection: &Value, raw_activity: LocalSpeakerTimeline) -> Self {
        let kind=|value:&Value| match value.as_str().unwrap() {"Owner"=>EvidenceKind::Owner,"Other"=>EvidenceKind::Other,_=>EvidenceKind::Unknown};
        let qualified_reference=projection["qualified_reference"].as_bool().unwrap();
        let rows=projection["source_rows"].as_array().unwrap().iter().map(|row|
            (row[0].as_u64().unwrap(),row[1].as_u64().unwrap(),kind(&row[2]))).collect();
        // Replay the captured decision, not a new model inference. Loading the
        // unchanged protected bank supplies lifecycle binding only; no natural
        // reference is fabricated or installed into the production profile.
        let binding=qualified_reference.then(|| (phrase.to_string(),
            speaker_verification::native_word_profile_for_phrase(phrase).unwrap().unwrap().binding_key()));
        Self {qualified_reference,rows,raw_activity,binding}
    }
    pub(crate) fn invalidate_if_profile_changed(&mut self) {
        if !self.binding.as_ref().is_some_and(|(phrase,key)|
            speaker_verification::native_word_binding_is_current(phrase,*key)) {
            self.qualified_reference = false;
            self.rows.clear();
        }
    }
    pub(super) fn kind(&self, row: &SourceRow) -> EvidenceKind {
        if !row.definite || row.speaker.is_none() { return EvidenceKind::Unknown; }
        // The provider can merge a whole paragraph into one native source.
        // Every complete word needs the same independently observed identity;
        // one short match cannot certify the rest of a long utterance.
        let mut identity = None;
        for (start, end) in row.word_ranges() {
            let Some((_, _, kind)) = self.rows.iter().find(|(begin, finish, _)|
                *begin <= start && end <= *finish) else { return EvidenceKind::Unknown; };
            if *kind == EvidenceKind::Unknown || identity.is_some_and(|old| old != *kind)
                || self.rows.iter().any(|(begin, finish, other)|
                    overlap(start, end, *begin, *finish) && other != kind) {
                return EvidenceKind::Unknown;
            }
            identity = Some(*kind);
        }
        identity.unwrap_or(EvidenceKind::Unknown)
    }

    pub(super) fn detector_excludes_owner(&self, start: u64, end: u64) -> bool {
        detector_observed_without_owner(&self.raw_activity, start, end)
    }
}

/// Complete raw detector observation is distinct from a rolling identity
/// rescue. Missing coverage and any actual detector positive veto absence.
pub(crate) fn detector_observed_without_owner(raw: &LocalSpeakerTimeline, start: u64, end: u64) -> bool {
        if start >= end || raw.presence.iter().any(|span|
            span.kind == EvidenceKind::Owner && overlap(start, end, span.start_ms, span.end_ms)) {
            return false;
        }
        let mut spans: Vec<_> = raw.presence.iter().filter(|span|
            span.kind == EvidenceKind::Unknown && overlap(start, end, span.start_ms, span.end_ms))
            .map(|span| (span.start_ms, span.end_ms)).collect();
        spans.sort_unstable();
        let mut frontier = start;
        for (begin, finish) in spans {
            if begin > frontier { return false; }
            frontier = frontier.max(finish);
            if frontier >= end { return true; }
        }
        false
}

fn overlap(a: u64, b: u64, c: u64, d: u64) -> bool { a < d && c < b }

/// Native separation must account for provisional rows too. A definite row
/// cannot borrow clean identity when another anonymous source overlaps it.
fn exclusive(row: &SourceRow, rows: &[SourceRow]) -> bool {
    row.definite && row.speaker.is_some() && rows.iter().all(|other|
        !overlap(row.start, row.end, other.start, other.end) || row.speaker == other.speaker)
}

fn exact_pcm(pcm: &[u8], start: u64, end: u64) -> Option<&[u8]> {
    if start >= end { return None; }
    let begin = usize::try_from(start).ok()?.checked_mul(32)?;
    let finish = usize::try_from(end).ok()?.checked_mul(32)?;
    pcm.get(begin..finish)
}

fn source_identity_ranges(row: &SourceRow) -> Option<Vec<(u64, u64)>> {
    let maximum = denzic_speaker_verification_v1_core::DEFAULT_MAX_CANDIDATE_MS as u64;
    let mut words = row.word_ranges();
    let (mut start, mut end) = words.next()?;
    if end.saturating_sub(start) > maximum { return None; }
    let mut ranges = Vec::new();
    for (word_start, word_end) in words {
        if word_end.saturating_sub(word_start) > maximum { return None; }
        if word_end.saturating_sub(start) > maximum {
            ranges.push((start, end));
            start = word_start;
        }
        end = word_end;
    }
    ranges.push((start, end));
    Some(ranges)
}

// Test-only acquisition-source contract: when this aligned definite
// response explicitly contains the configured wake prefix, a natural BODY
// exemplar cannot overlap that prefix. No duration/score threshold is changed.
fn explicit_wake_prefix_end(result: &Value, phrase: &str) -> Option<u64> {
    aligned_rows(result)?;
    let row=result.get("utterances")?.as_array()?.iter().find(|row|
        row.get("text").and_then(Value::as_str).is_some_and(|text|!crate::transcript_boundary::content_key(text).is_empty()))?;
    if row.get("definite") != Some(&Value::Bool(true)) {return None;}
    let wanted=crate::transcript_boundary::content_key(phrase);
    if wanted.is_empty(){return None;}
    let mut prefix=String::new();
    for word in row.get("words")?.as_array()? {
        prefix.push_str(&crate::transcript_boundary::content_key(word.get("text")?.as_str()?));
        if !wanted.starts_with(&prefix){return None;}
        if prefix==wanted {
            return ["end_time","endTime","end_ms"].iter().find_map(|key|word.get(*key).and_then(Value::as_u64));
        }
    }
    None
}

fn qualified_reference_ranges(rows: &[SourceRow], local: &LocalSpeakerTimeline,
    raw: &LocalSpeakerTimeline) -> Vec<(u64, u64)> {
    local.identity.iter().filter(|span| span.kind == EvidenceKind::Owner).filter_map(|span| {
        let touching: Vec<_> = rows.iter().filter(|row| overlap(span.start_ms, span.end_ms, row.start, row.end)).collect();
        let first = touching.first()?;
        if first.speaker.is_none() || touching.iter().any(|row| !row.definite || row.speaker != first.speaker) {
            return None;
        }
        let positive = touching.iter().any(|row| row.word_ranges().any(|(start, end)|
            raw.presence.iter().any(|activity| activity.kind == EvidenceKind::Owner
                && span.start_ms.max(start).max(activity.start_ms) < span.end_ms.min(end).min(activity.end_ms))));
        positive.then_some((span.start_ms, span.end_ms))
    }).collect()
}

#[derive(Default)]
pub(crate) struct NativeSourceIdentity {
    // Audio coordinates belong to this ASR capture. Cached embeddings cannot
    // survive reset; spelling/ID revisions always revalidate current rows.
    observations: BTreeMap<(u64, u64), SessionSpeakerObservation>,
    pub(super) natural_reference_after_wake: bool,
    pub(super) reference_acquisition_source: Option<(u64,u64)>,
}

impl NativeSourceIdentity {
    pub(crate) fn reset(&mut self) { self.observations.clear(); }

    pub(crate) fn analyze(&mut self, phrase: &str, result: &Value, pcm: &[u8],
        local: &LocalSpeakerTimeline, raw: LocalSpeakerTimeline) -> NativeWordIdentity {
        let mut output = NativeWordIdentity { raw_activity: raw, ..Default::default() };
        let Some(rows) = aligned_rows(result) else { return output; };
        let Ok(Some(mut profile)) = speaker_verification::native_word_profile_for_phrase(phrase) else { return output; };
        if !profile.has_qualified_reference {
            // A short first positive window may fail reference quality while a
            // later independent Owner window is usable. Rejections are source
            // observations, not a permanent claim on the acquisition slot.
            // Reuse measured failures and admit at most one new inference per
            // provider packet, preserving the existing receive-path work bound.
            let mut inferred_reference = false;
            let after_wake = self.natural_reference_after_wake.then(||explicit_wake_prefix_end(result,phrase)).flatten();
            for (start, end) in qualified_reference_ranges(&rows, local, &output.raw_activity).into_iter().filter(|(start,_)|after_wake.is_none_or(|end|*start>=end)) {
                if let Some(source) = exact_pcm(pcm, start, end) {
                    let key=(start,end);
                    if !self.observations.contains_key(&key) {
                        if inferred_reference { continue; }
                        inferred_reference = true;
                        if let Ok(observation)=profile.observe(source) { self.observations.insert(key,observation); }
                    }
                    if let Some(observation)=self.observations.get(&key) {
                        if speaker_verification::acquire_native_word_reference(phrase, &profile, observation) {
                            self.reference_acquisition_source=Some(key);
                            if let Ok(Some(updated)) = speaker_verification::native_word_profile_for_phrase(phrase) {
                                profile = updated;
                            }
                            break;
                        }
                    }
                }
            }
        }
        // No acquired natural reference: preserve the established product
        // behavior, including its conservative treatment of weak own speech.
        if !profile.has_qualified_reference { return output; }
        for row in rows.iter().filter(|row| exclusive(row, &rows)) {
            let Some(ranges) = source_identity_ranges(row) else { continue; };
            for key in ranges {
                if !self.observations.contains_key(&key) {
                    let Some(source) = exact_pcm(pcm, key.0, key.1) else { continue; };
                    let Ok(observation) = profile.observe(source) else { continue; };
                    self.observations.insert(key, observation);
                }
                let observation = &self.observations[&key];
                let kind = match profile.classify(observation) {
                    SessionSpeakerClassification::Target { .. } => EvidenceKind::Owner,
                    SessionSpeakerClassification::NonTarget { .. } if observation.signal_quality_sufficient => EvidenceKind::Other,
                    _ => EvidenceKind::Unknown,
                };
                output.rows.push((key.0, key.1, kind));
            }
        }
        if !speaker_verification::native_word_profile_is_current(phrase, &profile) {
            output.rows.clear();
            return output;
        }
        output.qualified_reference = true;
        output.binding = Some((phrase.to_string(), profile.binding_key()));
        output
    }
}


    }

    #[test]
    #[ignore = "explicit saved failed source/native projection; no device, cloud, editor or STOP"]
    fn long_native_owner_contract_with_fresh_source_evidence() {
        use sha2::Digest;
        use crate::asr::volcengine_speaker_ledger::{ConservativeSpeakerLedger, aligned_rows};
        let path=std::path::PathBuf::from(std::env::var_os("LISTENER_LONG_CONTRACT_INPUT").unwrap());
        let root=path.parent().unwrap();
        let fixture:Value=serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let phrase=fixture["phrase"].as_str().unwrap();
        assert!(speaker_verification::has_bound_template_without_mutation(phrase));
        let pcm=std::fs::read(fixture["pcm"].as_str().unwrap()).unwrap();
        assert_eq!(format!("{:x}",sha2::Sha256::digest(&pcm)),fixture["pcmSha256"].as_str().unwrap());
        let reference=std::fs::read(fixture["reference"].as_str().unwrap()).unwrap();
        assert_eq!(format!("{:x}",sha2::Sha256::digest(&reference)),fixture["referenceSha256"].as_str().unwrap());
        // Independent human-accepted quiet reference is fixed before BOTH
        // variants. RAM-only research precondition; not a product acquisition claim.
        let profile=speaker_verification::native_word_profile_for_phrase(phrase).unwrap().unwrap();
        let observation=profile.observe(&reference).unwrap();
        assert!(speaker_verification::acquire_native_word_reference(phrase,&profile,&observation));
        let decode=|name:&str| {
            let spans=|key:&str|fixture[name][key].as_array().unwrap().iter().map(|v|SpeakerSpan {
                start_ms:v["start_ms"].as_u64().unwrap(),end_ms:v["end_ms"].as_u64().unwrap(),
                kind:match v["kind"].as_str().unwrap(){"Owner"=>EvidenceKind::Owner,"Other"=>EvidenceKind::Other,_=>EvidenceKind::Unknown},
            }).collect();
            LocalSpeakerTimeline {identity:spans("identity"),presence:spans("presence")}
        };
        let local=decode("local");let raw=decode("raw");
        let trace:Value=serde_json::from_slice(&std::fs::read(fixture["trace"].as_str().unwrap()).unwrap()).unwrap();
        let entry=trace["trace"]["entries"].as_array().unwrap().iter().rev()
            .find(|e|e["stage"]=="normalized_result" && e["provider_result"].is_object()).unwrap();
        let result=&entry["provider_result"];
        let baseline=NativeSourceIdentity::default().analyze(phrase,result,&pcm,&local,raw.clone());
        let candidate=refreshed_long_contract::NativeSourceIdentity::default().analyze(phrase,result,&pcm,&local,raw.clone());
        let rows=aligned_rows(result).unwrap();
        let source_rows:Vec<_>=rows.iter().map(|row|json!([row.start,row.end,match candidate.kind(row){EvidenceKind::Owner=>"Owner",EvidenceKind::Other=>"Other",_=>"Unknown"}])).collect();
        let metadata=json!({"qualified_reference":candidate.qualified_reference,"source_rows":source_rows});
        let adapted=NativeWordIdentity::recorded_projection_for_test(phrase,&metadata,raw);
        let mut before=ConservativeSpeakerLedger::default();before.set_native_word_identity(baseline);
        let before=before.project_full_capture(result,&local,"");
        let mut after=ConservativeSpeakerLedger::default();after.set_native_word_identity(adapted);
        let after=after.project_full_capture(result,&local,"");
        let owner:Vec<_>=result["utterances"].as_array().unwrap().iter()
            .filter(|u|u["end_time"].as_u64().unwrap_or(u64::MAX)<=15200).filter_map(|u|u["text"].as_str()).collect();
        for text in &owner {
            assert!(crate::transcript_boundary::content_key(after.result["text"].as_str().unwrap())
                .contains(&crate::transcript_boundary::content_key(text)),"candidate removed known main owner body");
        }
        std::fs::write(root.join("result.json"),serde_json::to_vec_pretty(&json!({
            "scope":"Current baseline vs isolated old word-partition input contract, with current15 acquisition and fixed independent quiet reference; real100ms production source observations reused, final native packet only after all its audio observed. No field/live timing/editor/STOP or owner-return acceptance claim.",
            "sourceObservedThroughMs":fixture["sourceObservedThroughMs"],"originalFrame":entry["frame"],
            "candidateSourceRows":metadata,"baselineOwnerIds":before.owner_ids,"candidateOwnerIds":after.owner_ids,
            "baselineExcludedRows":before.excluded_rows,"candidateExcludedRows":after.excluded_rows,
            "baselineText":before.result["text"],"candidateText":after.result["text"],
            "knownMainOwnerPreserved":true,"knownOwnerRowCount":owner.len(),
            "newWeights":false,"cloudCalls":0,"actualEditor":false,"actualStop":false,
        })).unwrap()).unwrap();
    }

    #[test]
    #[ignore = "causal saved native callback/final arbitration with recorded readiness, no hardware/editor/cloud"]
    fn long_owner_input_contract_reaches_causal_callbacks() {
        use sha2::Digest;
        use crate::asr::volcengine::{VolcengineCredentials,VolcengineStreamingASR};
        use crate::asr::volcengine_speaker_ledger::aligned_rows;
        use std::sync::{Arc,Mutex};
        use std::sync::atomic::{AtomicU64,Ordering};
        use std::time::{Duration,Instant};
        let path=std::path::PathBuf::from(std::env::var_os("LISTENER_CAUSAL_LONG_INPUT").unwrap());
        let root=path.parent().unwrap();let fixture:Value=serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let phrase=fixture["phrase"].as_str().unwrap();assert!(speaker_verification::has_bound_template_without_mutation(phrase));
        let pcm=std::fs::read(fixture["pcm"].as_str().unwrap()).unwrap();assert_eq!(format!("{:x}",sha2::Sha256::digest(&pcm)),fixture["pcmSha256"].as_str().unwrap());
        let autonomous=fixture["preloadReference"]==false;
        let variant=fixture["onlyVariant"].as_str().unwrap_or("both");
        let profile=speaker_verification::native_word_profile_for_phrase(phrase).unwrap().unwrap();
        if autonomous {
            assert!(!profile.has_qualified_reference,"autonomous replay must start in a fresh process without a natural reference");
            assert!(variant=="baseline" || variant=="candidate");
        } else {
            let reference=std::fs::read(fixture["reference"].as_str().unwrap()).unwrap();assert_eq!(format!("{:x}",sha2::Sha256::digest(&reference)),fixture["referenceSha256"].as_str().unwrap());
            let obs=profile.observe(&reference).unwrap();assert!(speaker_verification::acquire_native_word_reference(phrase,&profile,&obs));
        }
        let decode=|name:&str| {
            let spans=|key:&str|fixture[name][key].as_array().unwrap().iter().map(|v|SpeakerSpan {
                start_ms:v["start_ms"].as_u64().unwrap(),end_ms:v["end_ms"].as_u64().unwrap(),
                kind:match v["kind"].as_str().unwrap(){"Owner"=>EvidenceKind::Owner,"Other"=>EvidenceKind::Other,_=>EvidenceKind::Unknown},
            }).collect();LocalSpeakerTimeline {identity:spans("identity"),presence:spans("presence")}
        };
        let all_local=decode("local");let all_raw=decode("raw");
        let clipped=|span:&SpeakerSpan,through:u64| (span.start_ms<through).then(||SpeakerSpan {end_ms:span.end_ms.min(through),..*span});
        let source_at=|through:u64| {
            let mut local=LocalSpeakerTimeline::default();let mut raw=LocalSpeakerTimeline::default();
            // Negative windows are available only once their actual model
            // source end has been observed. Future Owner revisions are retained
            // as extra vetoes; they can only reduce, never invent, exclusion.
            for span in all_local.identity.iter().filter(|s|s.end_ms<=through) {
                local.note_identity(*span);
                if span.kind==EvidenceKind::Other {local.note_presence(*span);}
            }
            for span in &all_raw.presence {
                if let Some(span)=clipped(span,through) {raw.note_presence(span);}
            }
            for span in all_local.presence.iter().chain(&all_local.identity).filter(|s|s.kind==EvidenceKind::Owner) {
                if let Some(span)=clipped(span,through) {local.note_presence(span);}
            }
            (local,raw)
        };
        let mut before=VolcengineStreamingASR::new(VolcengineCredentials {app_id:"offline".into(),access_token:"offline".into(),resource_id:"offline".into()},Vec::new()).with_owner_activity_only(true);
        let mut after=VolcengineStreamingASR::new(VolcengineCredentials {app_id:"offline".into(),access_token:"offline".into(),resource_id:"offline".into()},Vec::new()).with_owner_activity_only(true);
        let callbacks=Arc::new(Mutex::new(Vec::<Value>::new()));let current_frame=Arc::new(AtomicU64::new(0));
        for (name,asr) in [("baseline",&mut before),("candidate",&mut after)] {
            asr.note_accepted_wake_phrase(phrase);asr.enable_diagnostic_trace();
            let rows=Arc::clone(&callbacks);let frame=Arc::clone(&current_frame);
            asr.set_partial_transcript_callback(Some(Arc::new(move|text|rows.lock().unwrap().push(json!({"variant":name,"stage":"partial","frame":frame.load(Ordering::Relaxed),"text":text})))));
            let rows=Arc::clone(&callbacks);let frame=Arc::clone(&current_frame);
            asr.set_visual_partial_transcript_callback(Some(Arc::new(move|update|rows.lock().unwrap().push(json!({"variant":name,"stage":"visual","frame":frame.load(Ordering::Relaxed),"text":update.text()})))));
            let rows=Arc::clone(&callbacks);let frame=Arc::clone(&current_frame);
            asr.set_final_intermediate_transcript_callback(Some(Arc::new(move|update|rows.lock().unwrap().push(json!({"variant":name,"stage":"confirmed","frame":frame.load(Ordering::Relaxed),"text":update.text,"twoPass":update.authoritative_two_pass})))));
        }
        let trace:Value=serde_json::from_slice(&std::fs::read(fixture["trace"].as_str().unwrap()).unwrap()).unwrap();
        let mut baseline=NativeSourceIdentity::default();let mut candidate=refreshed_long_contract::NativeSourceIdentity::default();
        candidate.natural_reference_after_wake=fixture["postWakeReference"]==true;
        let began=Instant::now();let mut frames=Vec::new();let mut finals=Vec::new();
        for entry in trace["trace"]["entries"].as_array().unwrap().iter().filter(|e|e["stage"]=="normalized_result" && e["provider_result"].is_object()) {
            let due=Duration::from_millis(entry["elapsed_ms"].as_u64().unwrap());
            if let Some(wait)=due.checked_sub(began.elapsed()){std::thread::sleep(wait);}
            current_frame.store(entry["frame"].as_u64().unwrap(),Ordering::Relaxed);
            let original_floor=["local_identity_newest_first","local_presence_newest_first"].iter()
                .flat_map(|key|entry["speaker_projection"][*key].as_array().into_iter().flatten())
                .filter_map(|span|span[1].as_u64()).max().unwrap_or(0);
            let through=fixture["readinessObservations"].as_array().unwrap().iter()
                .filter(|o|o["capturedMs"].as_u64().unwrap()<=original_floor)
                .last().and_then(|o|o["analyzedMs"].as_u64()).unwrap_or(0);
            let (local,raw)=source_at(through);let result=&entry["provider_result"];
            let prefix=&pcm[..(original_floor as usize*32).min(pcm.len())];
            let timer=Instant::now();let current=if variant!="candidate" {baseline.analyze(phrase,result,prefix,&local,raw.clone())} else {NativeWordIdentity::default()};let baseline_model_ms=timer.elapsed().as_secs_f64()*1000.0;
            let timer=Instant::now();let proposed=if variant!="baseline" {candidate.analyze(phrase,result,prefix,&local,raw.clone())} else {refreshed_long_contract::NativeWordIdentity::default()};let candidate_model_ms=timer.elapsed().as_secs_f64()*1000.0;
            let reference_ready=speaker_verification::native_word_profile_for_phrase(phrase).unwrap().unwrap().has_qualified_reference;
            let rows=aligned_rows(result).unwrap_or_default();
            let source_rows:Vec<_>=rows.iter().map(|row|json!([row.start,row.end,
                match proposed.kind(row){EvidenceKind::Owner=>"Owner",EvidenceKind::Other if row.end-row.start<=10000=>"Other",_=>"Unknown"}])).collect();
            let metadata=json!({"qualified_reference":proposed.qualified_reference,"source_rows":source_rows});
            let identity=NativeWordIdentity::recorded_projection_for_test(phrase,&metadata,raw);
            before.install_native_word_identity_for_test(current);after.install_native_word_identity_for_test(identity);
            let final_frame=entry["final_frame"]==true;
            let a=before.apply_saved_native_result_with_local_timeline_for_test(result,final_frame,&local);
            let b=after.apply_saved_native_result_with_local_timeline_for_test(result,final_frame,&local);
            if let Some(output)=a {finals.push(json!({"variant":"baseline","text":output.text}));}
            if let Some(output)=b {finals.push(json!({"variant":"candidate","text":output.text}));}
            let snapshot=|asr:&VolcengineStreamingASR|asr.pause_early_delivery_evidence_after(Duration::from_millis(1000),"")
                .map(|s|json!({"text":s.text,"stability":format!("{:?}",s.stability)}));
            frames.push(json!({"frame":entry["frame"],"originalElapsedMs":entry["elapsed_ms"],"replayElapsedMs":began.elapsed().as_millis(),
                "recordedCapturedSourceLowerBoundMs":original_floor,"modelReadySourceMs":through,"candidateSourceRows":metadata,"candidateReferenceAcquisitionSource":candidate.reference_acquisition_source,
                "referenceQualifiedAfterFrame":reference_ready,"baselineModelMs":baseline_model_ms,"candidateModelMs":candidate_model_ms,"baselineDeliveryView":snapshot(&before),"candidateDeliveryView":snapshot(&after)}));
        }
        let before_trace=before.take_diagnostic_trace();let after_trace=after.take_diagnostic_trace();
        std::fs::write(root.join("causal-result.json"),serde_json::to_vec_pretty(&json!({
            "scope":fixture["scope"],"referenceScope":fixture["referenceScope"],"autonomousReference":autonomous,"onlyVariant":variant,"sourceNegativeReconstruction":"Only completed same-source bank Other windows plus every final Owner veto; source readiness and original native order/time preserved. No editor consumption or receipt manufactured.",
            "longNegativeAuthorizationAdded":false,"frames":frames,"callbacks":*callbacks.lock().unwrap(),"finals":finals,
            "baselineTrace":before_trace,"candidateTrace":after_trace,"elapsedSeconds":began.elapsed().as_secs_f64(),
            "newWeights":false,"cloudCalls":0,"actualEditor":false,"actualStop":false,
        })).unwrap()).unwrap();
    }
}
