//! Candidate nonblocking source producer. Ordinary ASR and original PCM
//! capture do not wait for these identity/model jobs.
use super::tracked_source_audio::{BoundSourceEvidence,TrackedSourceAudio};
use super::tracked_source_presence::{SourceEpoch,TrackedSourcePresence};
use super::volcengine_speaker_ledger::LocalSpeakerTimeline;
use crate::speaker_verification::NativeWordSpeakerProfile;
use parking_lot::{Mutex,Condvar};
use serde_json::Value;
use std::{collections::BTreeMap,path::PathBuf,sync::{Arc,atomic::{AtomicBool,AtomicU64,Ordering},mpsc},time::{Duration,Instant}};

enum Message { Audio(Vec<u8>,u64), Native(u64,Value,u64) }

struct NativeWork { revision:u64, pcm:Arc<Vec<u8>>, native:BTreeMap<u64,Value>, raw:LocalSpeakerTimeline }
#[derive(Default)]
struct NativeMailbox { pending:Option<NativeWork> }
struct NativeRoles { revision:u64, tracker:TrackedSourcePresence }
#[derive(Default)]
struct PrefixFrontier { captured_ms:u64, owner_wait_through_ms:u64, source_owned:bool }

#[derive(Clone,Debug,Default,serde::Serialize)]
pub(crate) struct SourceWorkerStatus {
    pub phase: &'static str,
    pub captured_ms: u64,
    pub owner_references: usize,
    pub foreign_references: usize,
    pub error: Option<String>,
    #[cfg(any(test, feature = "recorded-endpoint-probe"))]
    pub source_decisions: Option<Value>,
}

pub(crate) struct TrackedSourceWorker {
    sender:mpsc::SyncSender<Message>,
    stopped:Arc<AtomicBool>,
    prefix_needed:Arc<AtomicBool>,
    prefix_frontier:Arc<Mutex<PrefixFrontier>>,
    native_metadata_revision:Arc<AtomicU64>,
    native_submit_lock:Mutex<()>,
    status:Arc<Mutex<SourceWorkerStatus>>,
}

impl TrackedSourceWorker {
    pub(crate) fn start(epoch:SourceEpoch,phrase:String,profile:NativeWordSpeakerProfile,
        initial_pcm:Vec<u8>,model_path:PathBuf,raw:Arc<Mutex<crate::personal_vad::RawActivitySnapshot>>,
        output:Arc<Mutex<Option<BoundSourceEvidence>>>)->Self {
        let (sender,receiver)=mpsc::sync_channel(200);
        let stopped=Arc::new(AtomicBool::new(false));let failed=Arc::clone(&stopped);
        let status=Arc::new(Mutex::new(SourceWorkerStatus {phase:"starting",..Default::default()}));
        let worker_status=Arc::clone(&status);
        let prefix_needed=Arc::new(AtomicBool::new(false));let requests=Arc::clone(&prefix_needed);
        let prefix_frontier=Arc::new(Mutex::new(PrefixFrontier::default()));let frontier=Arc::clone(&prefix_frontier);
        let native_metadata_revision=Arc::new(AtomicU64::new(0));let metadata_clock=Arc::clone(&native_metadata_revision);
        let native_mailbox=Arc::new((Mutex::new(NativeMailbox::default()),Condvar::new()));
        let native_roles:Arc<Mutex<Option<NativeRoles>>>=Arc::new(Mutex::new(None));
        let history_mailbox=Arc::clone(&native_mailbox);let history_roles=Arc::clone(&native_roles);
        let history_stopped=Arc::clone(&stopped);let history_phrase=phrase.clone();let history_profile=profile.clone();
        // Historical complete-word verification owns its own actor. It cannot
        // make the current source observer wait for all words of an EOF reply.
        tauri::async_runtime::spawn_blocking(move || {
            let mut tracker=TrackedSourcePresence::new(epoch,&history_phrase,&history_profile);
            while !history_stopped.load(Ordering::Acquire) {
                let job={let (mailbox,wake)=&*history_mailbox;let mut mailbox=mailbox.lock();
                    if mailbox.pending.is_none(){wake.wait_for(&mut mailbox,Duration::from_millis(40));}
                    mailbox.pending.take()};
                let Some(job)=job else{continue;};
                if !crate::speaker_verification::native_word_profile_is_current(&history_phrase,&history_profile){break;}
                #[cfg(test)] let began=Instant::now();
                for (decoder,result) in &job.native {
                    if history_stopped.load(Ordering::Acquire){break;}
                    tracker.observe_native(epoch,*decoder,result,&job.pcm,&job.raw,&history_profile);
                }
                if history_stopped.load(Ordering::Acquire){break;}
                #[cfg(test)] eprintln!("[source-native-history] revision={} compute_ms={} captured_ms={}",job.revision,began.elapsed().as_millis(),job.pcm.len()/32);
                *history_roles.lock()=Some(NativeRoles{revision:job.revision,tracker:tracker.source_snapshot()});
            }
        });
        tauri::async_runtime::spawn_blocking(move || {
            let work=||->Result<(),String> {
                let mut pcm=initial_pcm;
                let mut native=BTreeMap::<u64,Value>::new();let mut audio=None;
                let mut last_pcm_size=0;let mut last_raw_owners=Vec::new();let mut last_raw_end=None;
                let mut native_revision=0;let mut observed_roles_revision=0;
                let mut observed_metadata_revision=0;
                loop {
                    if failed.load(Ordering::Acquire){break;}
                    let first=receiver.recv_timeout(Duration::from_millis(40));
                    let mut messages=Vec::new();
                    match first {Ok(message)=>messages.push(message),Err(mpsc::RecvTimeoutError::Timeout)=>(),Err(_)=>break}
                    messages.extend(receiver.try_iter());
                    let mut native_changed=false;let mut metadata_changed=false;
                    for message in messages {
                        match message {
                            Message::Audio(bytes,end_samples)=> {
                                if bytes.len()%2!=0 || (pcm.len()+bytes.len()) as u64!=end_samples*2 {return Err("source producer PCM discontinuity".into());}
                                pcm.extend(bytes);
                            },
                            Message::Native(decoder,result,revision)=>{native_changed|=native.get(&decoder)!=Some(&result);native.insert(decoder,result);observed_metadata_revision=observed_metadata_revision.max(revision);metadata_changed=true;},
                        }
                    }
                    if pcm.len()>60_000*32 {return Err("candidate source capture exceeds bounded integration contract".into());}
                    if !crate::speaker_verification::native_word_profile_is_current(&phrase,&profile){return Err("source profile expired".into());}
                    let raw_snapshot=raw.lock().clone();
                    let raw_end=raw_snapshot.completed_through_ms();
                    let raw_progress_changed=last_raw_end!=raw_end;last_raw_end=raw_end;
                    let raw=raw_snapshot.timeline;
                    let raw_owners:Vec<_>=raw.presence.iter().filter(|s|s.kind==super::volcengine_speaker_ledger::EvidenceKind::Owner).map(|s|(s.start_ms,s.end_ms)).collect();
                    let raw_changed=last_raw_owners!=raw_owners;
                    // Coordinates already inside immutable captured PCM need
                    // re-encoding only when provider metadata or raw owner
                    // protection changes, not on every 40ms audio message.
                    if native_changed || raw_changed {
                        native_revision+=1;
                        let (mailbox,wake)=&*native_mailbox;
                        mailbox.lock().pending=Some(NativeWork{revision:native_revision,pcm:Arc::new(pcm.clone()),native:native.clone(),raw:raw.clone()});
                        wake.notify_one();
                    }
                    last_raw_owners=raw_owners;
                    let captured_ms=pcm.len() as u64/32;
                    worker_status.lock().captured_ms=captured_ms;
                    frontier.lock().captured_ms=captured_ms;
                    let Some((roles_revision,mut tracker))=native_roles.lock().as_ref()
                        .map(|roles|(roles.revision,roles.tracker.source_snapshot())) else {
                            worker_status.lock().phase="native_history_pending";
                            last_pcm_size=pcm.len();continue;
                        };
                    if !native_changed && !metadata_changed && !raw_changed && !raw_progress_changed && last_pcm_size==pcm.len() && observed_roles_revision==roles_revision {continue;}
                    observed_roles_revision=roles_revision;
                    tracker.refresh_current_native(&native);
                    let counts=tracker.reference_counts();
                    {let mut status=worker_status.lock();status.owner_references=counts.0;status.foreign_references=counts.1;}
                    #[cfg(any(test, feature = "recorded-endpoint-probe"))]
                    {worker_status.lock().source_decisions=Some(tracker.diagnostic());}
                    #[cfg(test)]
                    if native_changed {eprintln!("[queued-source] captured_ms={captured_ms} references={counts:?} pcm_bytes={} raw_spans={}",pcm.len(),raw.presence.len());}
                    {let mut frontier=frontier.lock();
                        if !frontier.source_owned {frontier.owner_wait_through_ms=tracker.known_owner_end().unwrap_or_default();}}
                    if counts.0>0 && native.values().any(|r|r["utterances"].as_array().is_some_and(|rows|rows.len()>1)) {
                        requests.store(true,Ordering::Release);
                    }
                    if captured_ms<10_080 {
                        worker_status.lock().phase="source_support_pending";
                        last_pcm_size=pcm.len();continue;
                    }
                    // R32's eager bank encoding competed with the personal
                    // verifier before source handoff and delayed owner-return
                    // observations. Keep that failed scheduling experiment
                    // opt-in; ordinary candidate use retains the prior bounded
                    // role-first scheduling while the clock reconciliation fix
                    // remains independent of this computation choice.
                    if !tracker.has_foreign_reference()
                        && std::env::var_os("LISTENER_EARLY_SOURCE_OBSERVATION").is_none() {
                        worker_status.lock().phase="foreign_reference_pending";
                        last_pcm_size=pcm.len();continue;
                    }
                    if audio.is_none(){
                        #[cfg(test)] let load_began=Instant::now();
                        audio=Some(TrackedSourceAudio::load(&model_path,epoch)?);
                        #[cfg(test)] eprintln!("[queued-source] model_load_ms={}",load_began.elapsed().as_millis());
                    }
                    let audio=audio.as_mut().unwrap();
                    // The model directly observes ten seconds of original
                    // support. Native certificates reconcile only coordinates
                    // inside that observed support, avoiding a cold replay of
                    // every earlier window while live capture moves ahead.
                    // Match the existing 160ms activity quantum. A 480ms
                    // producer cannot satisfy a 250ms captured-audio freshness
                    // contract continuously, even when inference is instant.
                    if audio.captured_analysis_ms().is_none_or(|end|captured_ms.saturating_sub(end)>=160) {
                        worker_status.lock().phase="source_analysis_running";
                        #[cfg(test)] let source_began=Instant::now();
                        audio.analyze(epoch,&pcm,&profile)?;
                        #[cfg(test)] eprintln!("[queued-source] source_compute_ms={} captured_ms={captured_ms}",source_began.elapsed().as_millis());
                    }
                    last_pcm_size=pcm.len();
                    // Acoustic observation and source-role qualification have
                    // independent arrival clocks. Continuous foreign speech
                    // can delay a definite native row for tens of seconds.
                    // Preserve actual acoustic support while that identity is
                    // pending, then reconcile it when the qualified role arrives.
                    // Readiness alone must not publish absence or hand ownership
                    // away from the personal detector before foreign qualification.
                    if !tracker.has_foreign_reference() {
                        worker_status.lock().phase="foreign_reference_pending";
                        continue;
                    }
                    if let Some((frame,timeline))=audio.evidence(epoch,&tracker,&profile,&raw,Instant::now()) {
                        #[cfg(any(test, feature = "recorded-endpoint-probe"))]
                        {worker_status.lock().source_decisions=Some(audio.diagnostic(&tracker,&raw,&timeline));}
                        let source_analyzed_end_ms=frame.analyzed_end_ms;
                        let Some(frame)=BoundSourceEvidence::common_completed_frame(frame,raw_end) else {
                            worker_status.lock().phase="personal_completion_missing";
                            continue;
                        };
                        // Only a complete source observation may replace the
                        // source-owned horizon. Native bootstrap cannot reset
                        // it to the older activation while inference runs.
                        {let mut frontier=frontier.lock();frontier.source_owned=true;
                            frontier.owner_wait_through_ms=frame.last_positive_end_ms.max(frame.last_unresolved_end_ms).unwrap_or_default();}
                        if captured_ms.saturating_sub(frame.analyzed_end_ms)<=250 {
                            let mut published=output.lock();
                            if failed.load(Ordering::Acquire){break;}
                            *published=Some(BoundSourceEvidence {epoch,phrase:phrase.clone(),
                                profile_key:profile.binding_key(),captured_ms,source_analyzed_end_ms,frame,
                                #[cfg(test)] diagnostic:{let mut diagnostic=audio.diagnostic(&tracker,&raw,&timeline);
                                    diagnostic["historyRevision"]=roles_revision.into();diagnostic["requestedHistoryRevision"]=native_revision.into();diagnostic["rawAnalyzedEndMs"]=raw_end.into();diagnostic},
                                timeline,raw:raw.clone(),raw_analyzed_end_ms:raw_end,owner_ids:tracker.owner_ids_for(epoch.decoder),foreign_ids:tracker.foreign_ids_for(epoch.decoder),
                                metadata_revision:observed_metadata_revision,metadata_clock:Arc::clone(&metadata_clock)});
                            worker_status.lock().phase="source_published";
                            #[cfg(test)]
                            eprintln!("[queued-source] published captured_ms={captured_ms} analyzed_ms={}",frame.analyzed_end_ms);
                        }
                    } else {worker_status.lock().phase="source_reconciliation_pending";}
                }
                Ok(())
            };
            if let Err(error)=work(){log::warn!("[tracked-source] candidate unavailable: {error}");
                {let mut status=worker_status.lock();status.phase="source_failed";status.error=Some(error.clone());}
                #[cfg(test)] eprintln!("[queued-source] failed: {error}");failed.store(true,Ordering::Release);}
            failed.store(true,Ordering::Release);
        });
        Self{sender,stopped,prefix_needed,prefix_frontier,native_metadata_revision,native_submit_lock:Mutex::new(()),status}
    }
    pub(crate) fn status(&self)->SourceWorkerStatus {self.status.lock().clone()}
    pub(crate) fn submit_audio(&self,pcm:&[u8],end_samples:u64){
        if !self.stopped.load(Ordering::Acquire) && self.sender.try_send(Message::Audio(pcm.to_vec(),end_samples)).is_err(){self.stop();}
    }
    pub(crate) fn submit_native(&self,decoder:u64,result:Value){
        if !self.stopped.load(Ordering::Acquire) {
            // Sequence reservation and nonblocking enqueue share one order
            // across primary and child receivers. max(revision) therefore
            // means every earlier native message has actually been applied.
            let _serial=self.native_submit_lock.lock();
            let revision=self.native_metadata_revision.fetch_add(1,Ordering::AcqRel).wrapping_add(1);
            if self.sender.try_send(Message::Native(decoder,result,revision)).is_err(){self.stop();}
        }
    }
    pub(crate) fn prefix_requested(&self)->Option<(u64,u64)>{
        (!self.stopped.load(Ordering::Acquire) && self.prefix_needed.load(Ordering::Acquire)).then(||
            {let frontier=self.prefix_frontier.lock();(frontier.captured_ms,frontier.owner_wait_through_ms)})
    }
    pub(crate) fn stop(&self){self.stopped.store(true,Ordering::Release);}
}
impl Drop for TrackedSourceWorker {fn drop(&mut self){self.stop();}}
