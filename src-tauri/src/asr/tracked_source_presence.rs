//! Candidate source-role tracker. Text admission and recording dispatch stay
//! outside this module. Every reference belongs to one capture and protected
//! profile; an anonymous mismatch alone can never create a foreign reference.
use super::volcengine_speaker_ledger::{aligned_rows, EvidenceKind, LocalSpeakerTimeline};
use crate::speaker_verification::{NativeWordSpeakerProfile,SessionSpeakerClassification,SessionSpeakerObservation,SESSION_SPEAKER_CONFIDENT_TARGET_MIN_SCORE};
use serde_json::Value;
use std::collections::{BTreeMap,BTreeSet};

#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(crate) enum TrackedSourceDecision { Owner, CorroboratedForeign, Unknown }

#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(crate) struct SourceEpoch {pub capture:u64,pub capture_generation:u64,pub decoder:u64}

pub(crate) struct TrackedSourcePresence {
    epoch:SourceEpoch,
    phrase:String,
    profile_key:[u8;32],
    owners:BTreeSet<(u64,String)>,
    owner_references:Vec<SessionSpeakerObservation>,
    native_owner_ranges:BTreeMap<u64,Vec<(u64,u64)>>,
    native_unknown_ranges:Vec<(u64,u64)>,
    native_metadata_complete:bool,
    foreign_certificates:BTreeMap<u64,Vec<(u64,u64)>>,
    certified_seed_ranges:BTreeMap<u64,Vec<(u64,u64)>>,
    foreign:BTreeMap<(u64,String),SessionSpeakerObservation>,
    foreign_seeds:BTreeMap<(u64,String),(u64,u64)>,
    observations:BTreeMap<(u64,u64),SessionSpeakerObservation>,
}

impl TrackedSourcePresence {
    pub(crate) fn new(epoch:SourceEpoch,phrase:&str,profile:&NativeWordSpeakerProfile)->Self {
        Self{epoch,phrase:phrase.into(),profile_key:profile.binding_key(),owners:BTreeSet::new(),owner_references:Vec::new(),native_owner_ranges:BTreeMap::new(),native_unknown_ranges:Vec::new(),native_metadata_complete:true,foreign_certificates:BTreeMap::new(),certified_seed_ranges:BTreeMap::new(),foreign:BTreeMap::new(),foreign_seeds:BTreeMap::new(),observations:BTreeMap::new()}
    }
    fn current(&self,epoch:SourceEpoch,profile:&NativeWordSpeakerProfile)->bool {
        self.epoch==epoch && self.profile_key==profile.binding_key()
            && crate::speaker_verification::native_word_profile_is_current(&self.phrase,profile)
    }
    /// The current observer needs roles and certificates, not the historical
    /// encoder cache. Every embedding stays capture/profile scoped in RAM.
    pub(crate) fn source_snapshot(&self)->Self {
        Self{epoch:self.epoch,phrase:self.phrase.clone(),profile_key:self.profile_key,
            owners:self.owners.clone(),owner_references:self.owner_references.clone(),
            native_owner_ranges:self.native_owner_ranges.clone(),native_unknown_ranges:Vec::new(),native_metadata_complete:true,
            foreign_certificates:self.foreign_certificates.clone(),certified_seed_ranges:self.certified_seed_ranges.clone(),
            foreign:self.foreign.clone(),foreign_seeds:self.foreign_seeds.clone(),observations:BTreeMap::new()}
    }
    /// Apply the newest provider metadata without neural work. Historical
    /// jobs may still be processing a prior reply: current known-owner ranges
    /// supersede it immediately; explicitly unmapped voices retain Unknown.
    /// A provisional row with no identifier does not declare a new voice and
    /// cannot revoke independent, complete original-audio source evidence.
    pub(crate) fn refresh_current_native(&mut self,native:&BTreeMap<u64,Value>) {
        self.native_owner_ranges.clear();self.native_unknown_ranges.clear();self.native_metadata_complete=true;
        for (decoder,result) in native {
            let Some(rows)=aligned_rows(result) else{self.native_metadata_complete=false;continue;};
            for row in rows {
                let id=row.speaker.as_ref().map(|speaker|(*decoder,speaker.clone()));
                if id.as_ref().is_some_and(|id|self.owners.contains(id)) {
                    if row.definite {self.native_owner_ranges.entry(*decoder).or_default().push((row.start,row.end));}
                    else {self.native_unknown_ranges.push((row.start,row.end));}
                }else if id.as_ref().is_some_and(|id|!self.foreign.contains_key(id)) {
                    self.native_unknown_ranges.push((row.start,row.end));
                }
            }
        }
    }
    pub(crate) fn unresolved_native_overlaps(&self,start:u64,end:u64)->bool {
        !self.native_metadata_complete || self.native_unknown_ranges.iter().any(|(a,b)|*a<end && start<*b)
    }
    pub(crate) fn unresolved_native_ranges(&self)->&[(u64,u64)] {&self.native_unknown_ranges}
    /// Acquire from the first exclusive, definite, accurately timed source.
    /// Owner identity comes from the actual protected bank at the wake source,
    /// not an externally supplied speaker number or natural reference clip.
    pub(crate) fn observe_native(&mut self,epoch:SourceEpoch,native_decoder:u64,result:&Value,pcm:&[u8],
        raw:&LocalSpeakerTimeline,profile:&NativeWordSpeakerProfile)->bool {
        if !self.current(epoch,profile) {self.owners.clear();self.owner_references.clear();self.native_owner_ranges.clear();self.native_unknown_ranges.clear();self.native_metadata_complete=false;self.foreign_certificates.clear();self.certified_seed_ranges.clear();self.foreign.clear();self.foreign_seeds.clear();self.observations.clear();return false;}
        let Some(rows)=aligned_rows(result) else{return false;};
        let key=crate::transcript_boundary::content_key(&self.phrase);
        let exclusive=|index:usize| rows.iter().enumerate().all(|(j,other)|
            index==j || rows[index].end<=other.start || other.end<=rows[index].start
                || rows[index].speaker==other.speaker);
        let source=|start:u64,end:u64| usize::try_from(start).ok().and_then(|x|x.checked_mul(32))
            .zip(usize::try_from(end).ok().and_then(|x|x.checked_mul(32)))
            .and_then(|(a,b)|pcm.get(a..b));
        for (index,row) in rows.iter().enumerate().filter(|(_,r)|r.definite && r.speaker.is_some()) {
            // A local positive on a later ordinary row protects that audio;
            // it cannot rename an entire anonymous source as the owner. Such
            // promotion permanently poisoned foreign tracks after one false
            // positive. Only the independently verified activation anchors
            // this decoder's owner identifiers; continuation keeps its local
            // raw/original/partition positive vetoes in the audio reducer.
            if !exclusive(index) || !crate::transcript_boundary::content_key(&row.text).starts_with(&key){continue;}
            let id=(native_decoder,row.speaker.as_ref().unwrap().clone());
            if self.owners.contains(&id){continue;}
            let end=row.end.min(row.start+2400);
            if end-row.start<1000 || !raw.presence.iter().any(|s|s.kind==EvidenceKind::Owner && s.start_ms<end && row.start<s.end_ms){continue;}
            let Some(pcm)=source(row.start,end) else{continue;};
            let Some(observation)=cached_observation(&mut self.observations,profile,(row.start,end),pcm) else{continue;};
            if observation.signal_quality_sufficient && matches!(observation.classification,SessionSpeakerClassification::Target{..}) {
                self.foreign.remove(&id);self.foreign_seeds.remove(&id);self.owners.insert(id);
                if self.owner_references.len()<4 {self.owner_references.push(observation);}
            }
        }
        self.native_owner_ranges.insert(native_decoder,rows.iter().filter(|row|row.definite
            && row.speaker.as_ref().is_some_and(|id|self.owners.contains(&(native_decoder,id.clone()))))
            .map(|r|(r.start,r.end)).collect());
        // Enrollment and decoder ID naming are different responsibilities.
        // The body stream may start after the verified wake phrase was cut.
        // Its current immutable protected bank remains an owner anchor even
        // when no provider ID can be named from that missing phrase. Keep all
        // raw/native Owner vetoes and the same independent foreign-seed checks;
        // enrollment alone never turns an anonymous number into Owner/Other.
        if self.owners.is_empty() && !profile.has_protected_source_anchor(){return false;}
        let before=self.foreign.len();
        for (index,row) in rows.iter().enumerate().filter(|(_,r)|r.definite && r.speaker.is_some()) {
            let id=(native_decoder,row.speaker.as_ref().unwrap().clone());
            if self.owners.contains(&id) || self.foreign.contains_key(&id) || !exclusive(index) {continue;}
            let words:Vec<_>=row.word_ranges().collect();
            // An EOF decoder numbers the same retained PCM again. Its number
            // is an alias, not another acoustic reference or a new voice.
            // Reuse only an exactly contained certified seed in this capture,
            // after rechecking current raw/native owner protection. A source
            // similarity or a matching number cannot establish this alias.
            let reused=self.foreign_seeds.iter().find_map(|(known,range)| {
                let (start,end)=*range;
                let contained=words.first().is_some_and(|(a,_)|*a<=start)
                    && words.last().is_some_and(|(_,b)|end<=*b)
                    && row.start<=start && end<=row.end;
                (contained && !self.known_owner_overlaps(start,end)
                    && super::volcengine_native_identity::detector_observed_without_owner(raw,start,end))
                    .then(||self.foreign.get(known).filter(|o|o.signal_quality_sufficient
                        && matches!(o.classification,SessionSpeakerClassification::NonTarget{..})))
                    .flatten().map(|observation|(*range,observation.clone()))
            });
            if let Some((range,observation))=reused {
                self.certified_seed_ranges.entry(native_decoder).or_default().push(range);
                self.foreign_seeds.insert(id.clone(),range);self.foreign.insert(id,observation);
                continue;
            }
            if self.unique_foreign_reference_count()>=3 {continue;}
            // Bootstrap retains the original first qualified bounded seed.
            // Longer windows can mix a returning owner into this source and
            // prevent a reference that the shorter exact window qualifies.
            'seed: for (i,(start,_)) in words.iter().enumerate() {
                for (_,end) in words.iter().skip(i) {
                    // A long mixed row can average a short owner return away.
                    // Freeze a bounded complete-word seed, never that average.
                    if end-start>2400 {break;}
                    if end-start<1000 || !super::volcengine_native_identity::detector_observed_without_owner(raw,*start,*end){continue;}
                    let Some(pcm)=source(*start,*end) else{continue;};
                    let Some(observation)=cached_observation(&mut self.observations,profile,(*start,*end),pcm) else{continue;};
                    let Some(context)=source(end.saturating_sub(2400),*end) else{continue;};
                    let Some(context)=cached_observation(&mut self.observations,profile,(end.saturating_sub(2400),*end),context) else{continue;};
                    if !context.signal_quality_sufficient || !matches!(context.classification,SessionSpeakerClassification::NonTarget{..}){continue;}
                    if observation.signal_quality_sufficient && matches!(observation.classification,SessionSpeakerClassification::NonTarget{..}) {
                        self.certified_seed_ranges.entry(native_decoder).or_default().push((*start,*end));
                        self.foreign_seeds.insert(id.clone(),(*start,*end));
                        self.foreign.insert(id.clone(),observation);break 'seed;
                    }
                }
            }
        }
        let mut certificates=self.certified_seed_ranges.get(&native_decoder).cloned().unwrap_or_default();
        for (index,row) in rows.iter().enumerate().filter(|(_,r)|r.definite && r.speaker.is_some()) {
            let id=(native_decoder,row.speaker.as_ref().unwrap().clone());
            if !self.foreign.contains_key(&id) || self.owners.contains(&id) || !exclusive(index){continue;}
            let words:Vec<_>=row.word_ranges().collect();
            for (start,end) in bounded_word_support(&words) {
                    if self.known_owner_overlaps(start,end){continue;}
                    let Some(pcm)=source(start,end) else {continue;};
                    let Some(observed)=cached_observation(&mut self.observations,profile,(start,end),pcm) else {continue;};
                    if observed.signal_quality_sufficient
                        && matches!(observed.classification,SessionSpeakerClassification::NonTarget{..})
                        && !self.owner_references.iter().any(|r|observed.source_similarity(r).is_some_and(|s|s>=SESSION_SPEAKER_CONFIDENT_TARGET_MIN_SCORE))
                        && self.foreign.values().any(|r|observed.source_similarity(r).is_some_and(|s|s>=SESSION_SPEAKER_CONFIDENT_TARGET_MIN_SCORE)) {
                        certificates.push((start,end));
                    }
            }
        }
        self.foreign_certificates.insert(native_decoder,certificates);
        self.foreign.len()>before
    }
    pub(crate) fn has_foreign_reference(&self)->bool {!self.foreign.is_empty()}
    fn unique_foreign_reference_count(&self)->usize {self.foreign_seeds.values().copied().collect::<BTreeSet<_>>().len()}
    pub(crate) fn reference_counts(&self)->(usize,usize){(self.owners.len(),self.unique_foreign_reference_count())}
    #[cfg(any(test, feature = "recorded-endpoint-probe"))]
    pub(crate) fn diagnostic(&self)->Value {
        serde_json::json!({"counts":self.reference_counts(),"ownerIds":self.owners.iter().collect::<Vec<_>>(),
            "foreignIds":self.foreign.keys().collect::<Vec<_>>(),"foreignReferenceSeeds":self.foreign_seeds.iter().collect::<Vec<_>>(),"nativeOwnerRanges":self.native_owner_ranges,
            "seedRanges":self.certified_seed_ranges,"certificates":self.foreign_certificates,
            "unresolvedNativeRanges":self.native_unknown_ranges,"nativeMetadataComplete":self.native_metadata_complete})
    }
    #[cfg(any(test, feature = "recorded-endpoint-probe"))]
    pub(crate) fn reference_matches(&self, observed: &SessionSpeakerObservation) -> Value {
        serde_json::json!({"owner":self.owner_references.iter()
            .filter_map(|r|observed.source_similarity(r)).collect::<Vec<_>>(),
            "foreign":self.foreign.iter().map(|(id,r)|serde_json::json!({"id":id,
                "similarity":observed.source_similarity(r)})).collect::<Vec<_>>()})
    }
    pub(crate) fn known_owner_overlaps(&self,start:u64,end:u64)->bool {
        self.native_owner_ranges.values().flatten().any(|(a,b)|*a<end && start<*b)
    }
    pub(crate) fn known_owner_end(&self)->Option<u64>{self.native_owner_ranges.values().flatten().map(|(_,end)|*end).max()}
    pub(crate) fn owner_ids_for(&self,decoder:u64)->Vec<String>{self.owners.iter().filter(|(id,_)|*id==decoder).map(|(_,speaker)|speaker.clone()).collect()}
    pub(crate) fn foreign_ids_for(&self,decoder:u64)->Vec<String>{self.foreign.keys().filter(|(id,_)|*id==decoder).map(|(_,speaker)|speaker.clone()).collect()}
    pub(crate) fn foreign_certificates(&self)->Vec<(u64,u64)> {
        self.foreign_certificates.values().flatten().copied().collect()
    }
    pub(crate) fn partitions_support_foreign(&self,partitions:&[SessionSpeakerObservation])->bool {
        !partitions.is_empty() && partitions.iter().all(|o|o.signal_quality_sufficient
            && !matches!(o.classification,SessionSpeakerClassification::Target{..})
            && !self.owner_references.iter().any(|r|o.source_similarity(r).is_some_and(|s|s>=SESSION_SPEAKER_CONFIDENT_TARGET_MIN_SCORE))
            && self.foreign.values().any(|r|o.source_similarity(r).is_some_and(|s|s>=SESSION_SPEAKER_CONFIDENT_TARGET_MIN_SCORE)))
    }
    pub(crate) fn source_negatives_corroborate_track(&self,original:&SessionSpeakerObservation,
        partitions:&[SessionSpeakerObservation])->bool {
        self.foreign_certificates.values().any(|ranges|!ranges.is_empty())
            && original.signal_quality_sufficient
            && matches!(original.classification,SessionSpeakerClassification::NonTarget{..})
            && partitions.iter().all(|p|matches!(p.classification,SessionSpeakerClassification::NonTarget{..}))
            && self.partitions_support_foreign(partitions)
            && self.foreign.values().any(|r|original.source_similarity(r).is_some_and(|s|s>=SESSION_SPEAKER_CONFIDENT_TARGET_MIN_SCORE))
            && !self.owner_references.iter().any(|r|original.source_similarity(r).is_some_and(|s|s>=SESSION_SPEAKER_CONFIDENT_TARGET_MIN_SCORE))
    }
    pub(crate) fn native_owner_overlaps(&self,native_decoder:u64,result:&Value,start:u64,end:u64)->bool {
        aligned_rows(result).is_some_and(|rows|rows.iter().any(|row|
            row.definite && row.start<end && start<row.end && row.speaker.as_ref().is_some_and(|id|self.owners.contains(&(native_decoder,id.clone())))))
    }
    /// This is source-role evidence, not complete original-audio absence.
    /// The caller must separately retain exact segmentation coverage, native
    /// owner words, source epochs, delivery obligations and real receipts.
    pub(crate) fn decide(&self,epoch:SourceEpoch,original:&SessionSpeakerObservation,
        partitions:&[SessionSpeakerObservation],profile:&NativeWordSpeakerProfile)->TrackedSourceDecision {
        if !self.current(epoch,profile){return TrackedSourceDecision::Unknown;}
        let owner=|o:&SessionSpeakerObservation|o.signal_quality_sufficient
            && (matches!(o.classification,SessionSpeakerClassification::Target{..})
                || self.owner_references.iter().any(|known|o.source_similarity(known).is_some_and(|s|s>=SESSION_SPEAKER_CONFIDENT_TARGET_MIN_SCORE)));
        if owner(original) || partitions.iter().any(owner){return TrackedSourceDecision::Owner;}
        let other=|o:&SessionSpeakerObservation|o.signal_quality_sufficient
            && !matches!(o.classification,SessionSpeakerClassification::Target{..})
            && self.foreign.values().any(|known|o.source_similarity(known).is_some_and(|s|s>=SESSION_SPEAKER_CONFIDENT_TARGET_MIN_SCORE));
        if !partitions.is_empty() && other(original) && partitions.iter().all(other) {
            TrackedSourceDecision::CorroboratedForeign
        } else {TrackedSourceDecision::Unknown}
    }
}

/// Cover a native row with bounded, complete-word windows, independent of
/// identity scores. Enumerating every word subsequence delayed the first
/// current-source observation by seconds in the real queue. Adjacent supports
/// overlap only at actual word coordinates; an unqualified window stays a gap.
/// The trailing short support is anchored backwards, never padded with silence.
fn bounded_word_support(words:&[(u64,u64)])->Vec<(u64,u64)> {
    let mut supports=Vec::new();let mut first=0;
    while first<words.len() {
        let start=words[first].0;
        let Some(last)=(first..words.len()).take_while(|i|words[*i].1.saturating_sub(start)<=2400).last() else {first+=1;continue;};
        let end=words[last].1;
        if end.saturating_sub(start)>=1000 {supports.push((start,end));}
        if last+1==words.len(){
            if end.saturating_sub(start)<1000 {
                if let Some(back)=(0..first).find(|i|(1000..=2400).contains(&end.saturating_sub(words[*i].0))) {
                    supports.push((words[back].0,end));
                }
            }
            break;
        }
        // Reuse the last word as real overlap where it permits progress.
        first=if last>first {last}else{first+1};
    }
    supports
}

fn cached_observation(cache:&mut BTreeMap<(u64,u64),SessionSpeakerObservation>,profile:&NativeWordSpeakerProfile,
    range:(u64,u64),pcm:&[u8])->Option<SessionSpeakerObservation> {
    if !cache.contains_key(&range) { cache.insert(range,profile.observe(pcm).ok()?); }
    cache.get(&range).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::volcengine_speaker_ledger::SpeakerSpan;
    use crate::personal_vad::{PresenceEndpoint,PresenceObservation,PresenceState};
    use sha2::{Digest,Sha256};
    use std::time::{Duration,Instant};

    fn feature(cache:&mut BTreeMap<String,SessionSpeakerObservation>,profile:&NativeWordSpeakerProfile,
        value:&Value)->SessionSpeakerObservation {
        let hash=value["sha256"].as_str().unwrap();
        if !cache.contains_key(hash) {
            let pcm=std::fs::read(value["pcm"].as_str().unwrap()).unwrap();
            assert_eq!(format!("{:x}",Sha256::digest(&pcm)),hash);
            cache.insert(hash.into(),profile.observe(&pcm).unwrap());
        }
        cache[hash].clone()
    }

    #[test]
    #[ignore="causal saved failed audio and actual ordinary ASR arrivals; current protected bank read only"]
    fn recorded_source_tracking_follows_real_reference_availability_and_retains_owner_return() {
        let root=std::path::PathBuf::from(std::env::var_os("LISTENER_COMPLETE_SOURCE_REPLAY").unwrap());
        let selected=std::env::var("LISTENER_COMPLETE_SOURCE_CASE").unwrap();
        let protocol_file=if selected.starts_with("late-"){"source-protocol-late.json"}else{"source-protocol.json"};
        let protocol:Value=serde_json::from_slice(&std::fs::read(root.join(protocol_file)).unwrap()).unwrap();
        let case=protocol["cases"].as_array().unwrap().iter().find(|c|c["id"]==selected).unwrap();
        let phrase=protocol["phrase"].as_str().unwrap();
        let profile=crate::speaker_verification::native_word_profile_for_phrase(phrase).unwrap().unwrap();
        assert!(!profile.has_qualified_reference,"must not borrow a natural owner reference");
        let pcm=std::fs::read(case["pcm"].as_str().unwrap()).unwrap();
        assert_eq!(format!("{:x}",Sha256::digest(&pcm)),case["pcmSha256"].as_str().unwrap());
        let native:Value=serde_json::from_slice(&std::fs::read(case["native"].as_str().unwrap()).unwrap()).unwrap();
        let nomo:Value=serde_json::from_slice(&std::fs::read(case["nomo"].as_str().unwrap()).unwrap()).unwrap();
        let mut frames:Vec<_>=native["frames"].as_array().unwrap().iter().map(|f|(1_u64,f.clone())).collect();
        let prefix=root.join(format!("{}-prefix-native.json",selected));
        if prefix.exists() {
            let prefix:Value=serde_json::from_slice(&std::fs::read(prefix).unwrap()).unwrap();
            assert!(prefix["logicalEofOnly"].as_bool().unwrap());
            frames.extend(prefix["frames"].as_array().unwrap().iter().map(|f|(2_u64,f.clone())));
        }
        frames.sort_by(|a,b|a.1["capturedMsAtReceipt"].as_f64().unwrap().total_cmp(&b.1["capturedMsAtReceipt"].as_f64().unwrap()));
        let scores=nomo["windows"].as_array().unwrap();
        let epoch=SourceEpoch{capture:1,capture_generation:1,decoder:1};
        let mut tracker=TrackedSourcePresence::new(epoch,phrase,&profile);
        let mut raw=LocalSpeakerTimeline::default();let mut ni=0;let mut pi=0;
        let mut cache=BTreeMap::new();let mut rows=Vec::new();let mut acquisitions=Vec::new();
        let mut latest_native=serde_json::json!({});
        let began=Instant::now();let mut clock=PresenceEndpoint::default();
        let mut owner_end=None;let mut unresolved_end=None;
        let mut first_source_due=None;
        for step in case["steps"].as_array().unwrap() {
            let captured=step["capturedMs"].as_u64().unwrap();
            while pi<scores.len() && scores[pi]["endMs"].as_u64().unwrap()<=captured {
                let end=scores[pi]["endMs"].as_u64().unwrap();
                for (i,p) in scores[pi]["probabilities"].as_array().unwrap().iter().enumerate() {
                    let finish=end-(14-i) as u64*160;
                    raw.note_presence(SpeakerSpan{start_ms:finish-160,end_ms:finish,kind:EvidenceKind::Unknown});
                    if p.as_f64().unwrap()>=0.7 {raw.note_presence(SpeakerSpan{start_ms:finish-160,end_ms:finish,kind:EvidenceKind::Owner});}
                }
                pi+=1;
            }
            while ni<frames.len() && frames[ni].1["capturedMsAtReceipt"].as_f64().unwrap()<=captured as f64 {
                let (decoder,frame)=&frames[ni];
                if *decoder==1 {latest_native=frame["provider_result"].clone();}
                let before=tracker.reference_counts();
                tracker.observe_native(epoch,*decoder,&frame["provider_result"],&pcm[..captured as usize*32],&raw,&profile);
                let after=tracker.reference_counts();
                if before!=after {acquisitions.push(serde_json::json!({"capturedMs":captured,"nativeDecoder":decoder,"receiptMs":frame["elapsed_ms"],"before":before,"after":after}));}
                ni+=1;
            }
            let original=feature(&mut cache,&profile,&step["original"]);
            let mut partitions=Vec::new();let mut incomplete=step["anyUnobservedOverlapVoice"].as_bool().unwrap();
            for part in step["partitions"].as_array().unwrap() {
                if part["insufficient"].as_bool()==Some(true) {incomplete=true;}
                else {partitions.push(feature(&mut cache,&profile,part));}
            }
            let decision=tracker.decide(epoch,&original,&partitions,&profile);
            let start=step["sourceRangeSamples"][0].as_u64().unwrap()/16;
            let end=step["sourceRangeSamples"][1].as_u64().unwrap()/16;
            let raw_owner=raw.presence.iter().any(|s|s.kind==EvidenceKind::Owner && s.start_ms<end && start<s.end_ms);
            let native_owner=tracker.native_owner_overlaps(1,&latest_native,start,end);
            let state=if raw_owner || native_owner || decision==TrackedSourceDecision::Owner {PresenceState::Owner}
                else if !incomplete && decision==TrackedSourceDecision::CorroboratedForeign {PresenceState::ConfirmedOther}
                else {PresenceState::Unknown};
            match state {PresenceState::Owner=>owner_end=Some(end),PresenceState::Unknown=>unresolved_end=Some(end),_=>()}
            let now=began+Duration::from_millis(captured);
            clock.observe(PresenceObservation{analyzed_end_ms:end,last_positive_end_ms:owner_end,
                last_unresolved_end_ms:unresolved_end,source_history_complete:true,state,observed_at:now});
            // This deliberately supplies no invented editor receipt. It only
            // measures source eligibility; production delivery must still settle.
            let due=clock.due(captured,now,3000);
            if due && first_source_due.is_none(){first_source_due=Some(captured);}
            rows.push(serde_json::json!({"capturedMs":captured,"sourceRangeMs":[start,end],
                "originalScore":original.classification.score(),"originalClass":format!("{:?}",original.classification),
                "partitionScores":partitions.iter().map(|p|p.classification.score()).collect::<Vec<_>>(),
                "originalOwnerReferenceMatches":tracker.owner_references.iter().filter_map(|r|original.source_similarity(r)).collect::<Vec<_>>(),
                "originalForeignReferenceMatches":tracker.foreign.values().filter_map(|r|original.source_similarity(r)).collect::<Vec<_>>(),
                "decision":format!("{:?}",decision),"rawOwnerVeto":raw_owner,"nativeOwnerVeto":native_owner,
                "incompletePartitionCoverage":incomplete,"fusedState":format!("{:?}",state),
                "ownerFrontierMs":owner_end,"unknownFrontierMs":unresolved_end,"sourceEligible":due}));
        }
        assert!(crate::speaker_verification::native_word_profile_is_current(phrase,&profile));
        let return_range=case["ownerReturnRangeMsAssessmentOnly"].as_array();
        let premature_source_due=!selected.starts_with("late-") && return_range.is_some_and(|r|first_source_due.is_some_and(|due|due<=r[1].as_u64().unwrap()));
        let eligible_during_return:Vec<_>=rows.iter().filter(|row|row["sourceEligible"].as_bool()==Some(true)
            && return_range.is_some_and(|r|row["sourceRangeMs"][1].as_u64().unwrap()>r[0].as_u64().unwrap()
                && row["sourceRangeMs"][0].as_u64().unwrap()<r[1].as_u64().unwrap())).map(|r|r["capturedMs"].clone()).collect();
        let mut late_identity_counterexamples=Vec::new();
        if selected=="continuous-foreign" {
            // These returns share this capture's actual prefix through 26791ms.
            // Both frozen references were acquired at 22080ms and never revised.
            // They test identity after bootstrap, not a duty to keep recording
            // until a return eleven seconds after the owner originally stopped.
            assert_eq!(acquisitions.len(),1);
            let old_root=root.parent().unwrap().join("protected-partition");
            let old:Value=serde_json::from_slice(&std::fs::read(old_root.join("protocol.json")).unwrap()).unwrap();
            let mut seen=BTreeSet::new();
            for row in old["rows"].as_array().unwrap() {
                let observed=row["observedUntilMs"].as_u64().unwrap();
                if observed<22080 || !seen.insert(observed) {continue;}
                let samples=row["samples"].as_array().unwrap();
                let original=feature(&mut cache,&profile,&samples[0]);
                let partitions:Vec<_>=samples[1..].iter().map(|s|feature(&mut cache,&profile,s)).collect();
                let decision=tracker.decide(epoch,&original,&partitions,&profile);
                late_identity_counterexamples.push(serde_json::json!({"id":row["id"],"observedUntilMs":observed,
                    "decision":format!("{:?}",decision),"originalScore":original.classification.score(),
                    "ownerMatches":tracker.owner_references.iter().filter_map(|r|original.source_similarity(r)).collect::<Vec<_>>(),
                    "foreignMatches":tracker.foreign.values().filter_map(|r|original.source_similarity(r)).collect::<Vec<_>>(),
                    "identityOnly":true,"newPhysicalStop":false}));
            }
        }
        std::fs::write(root.join(format!("{}-source-result-v4.json",selected)),serde_json::to_vec_pretty(&serde_json::json!({
            "case":selected,"acquisitions":acquisitions,"references":tracker.reference_counts(),
            "firstSourceEligibleCapturedMs":first_source_due,"prematureSourceEligibleBeforeReturnEnd":premature_source_due,
            "sourceOnly":true,"editorReceiptsFabricated":false,"actualStop":false,"actualInstalled":false,
            "encodedNativeSources":tracker.observations.iter().map(|(range,o)|serde_json::json!({"rangeMs":range,"score":o.classification.score(),"class":format!("{:?}",o.classification),"realSpeechMs":o.real_speech_ms,"quality":o.signal_quality_sufficient})).collect::<Vec<_>>(),
            "lateIdentityCounterexamples":late_identity_counterexamples,
            "sourceEligibleDuringKnownOwnerReturn":eligible_during_return,
            "computeMs":began.elapsed().as_millis(),"rows":rows})).unwrap()).unwrap();
        assert!(!premature_source_due,"source route would finish before known owner continuation completes");
        assert!(eligible_during_return.is_empty(),"source route remains eligible while known owner speech has returned");
    }
}
