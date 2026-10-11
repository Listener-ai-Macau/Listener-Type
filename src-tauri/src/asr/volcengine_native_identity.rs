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

fn qualified_reference_range(rows: &[SourceRow], local: &LocalSpeakerTimeline,
    raw: &LocalSpeakerTimeline) -> Option<(u64, u64)> {
    local.identity.iter().filter(|span| span.kind == EvidenceKind::Owner).find_map(|span| {
        let touching: Vec<_> = rows.iter().filter(|row| overlap(span.start_ms, span.end_ms, row.start, row.end)).collect();
        let first = touching.first()?;
        if first.speaker.is_none() || touching.iter().any(|row| !row.definite || row.speaker != first.speaker) {
            return None;
        }
        let positive = touching.iter().any(|row| row.word_ranges().any(|(start, end)|
            raw.presence.iter().any(|activity| activity.kind == EvidenceKind::Owner
                && span.start_ms.max(start).max(activity.start_ms) < span.end_ms.min(end).min(activity.end_ms))));
        positive.then_some((span.start_ms, span.end_ms))
    })
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
            if let Some((start, end)) = qualified_reference_range(&rows, local, &output.raw_activity) {
                if let Some(source) = exact_pcm(pcm, start, end) {
                    let key=(start,end);
                    if !self.observations.contains_key(&key) {
                        if let Ok(observation)=profile.observe(source) { self.observations.insert(key,observation); }
                    }
                    if let Some(observation)=self.observations.get(&key) {
                        if speaker_verification::acquire_native_word_reference(phrase, &profile, observation) {
                            if let Ok(Some(updated)) = speaker_verification::native_word_profile_for_phrase(phrase) {
                                profile = updated;
                            }
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
        assert_eq!(qualified_reference_range(&clean,&local,&raw),None,"rolling identity cannot stand in for the detector");
        raw.note_presence(SpeakerSpan {start_ms:200,end_ms:400,kind:EvidenceKind::Owner});
        assert_eq!(qualified_reference_range(&clean,&local,&raw),Some((0,1400)));
        for other_definite in [true,false] {
            let mixed=aligned_rows(&packet(vec![row("Owner","1",100,1800,true),row("Other","2",900,1900,other_definite)])).unwrap();
            assert_eq!(qualified_reference_range(&mixed,&local,&raw),None);
            assert!(!exclusive(&mixed[0],&mixed));
        }
        assert!(exact_pcm(&[0;32],0,2).is_none(),"no padding or clamping of unavailable source");
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
}
