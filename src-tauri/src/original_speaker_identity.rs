//! Rejected diagnostic candidate at the existing 1.2 s / 400 ms sampling
//! cadence. The actual quiet continuation regression fails. This test-only
//! module must not supply production submission or enrollment permission.

use crate::{speaker_identity_eres::{EresEmbedding, EresRuntime}, speech_vad_windows::SourceSpeechQuality,
    word_authority::{AudioSource, IdentityCell, WordAuthority, WordIdentity}};
use parking_lot::Mutex;
use std::{sync::{Arc, atomic::{AtomicBool, Ordering}, mpsc}, time::Instant};

// Same acoustic sampling cadence as the existing session verifier. This
// changes provenance and permission, not the endpoint or a symptom timer.
const CELL_MS: u64 = 400;
const CONTEXT_MS: u64 = 400;
const OPERATING_POINT: f32 = 0.36;

pub(crate) struct OriginalIdentityAnalyzer {
    runtime: Arc<Mutex<EresRuntime>>,
    reference: EresEmbedding,
    quality: SourceSpeechQuality,
    pcm: Vec<u8>,
    pub authority: WordAuthority,
}

impl OriginalIdentityAnalyzer {
    pub(crate) fn new(source: AudioSource, reference: EresEmbedding, runtime: Arc<Mutex<EresRuntime>>) -> Result<Self,String> {
        Ok(Self { runtime,reference,quality:SourceSpeechQuality::new()?,pcm:Vec::new(),
            authority:WordAuthority::new(source) })
    }

    pub(crate) fn accept(&mut self, pcm: &[u8]) -> Result<(),String> {
        if self.authority.complete { return Err("identity input arrived after completion".into()); }
        if pcm.len() % 2 != 0 { return Err("identity PCM sample is incomplete".into()); }
        self.pcm.extend_from_slice(pcm);
        self.quality.accept(pcm)?;
        self.analyze(false)
    }

    pub(crate) fn finish(&mut self) -> Result<(),String> {
        self.quality.finish();
        self.analyze(true)?;
        self.authority.complete = true;
        self.authority.settled_through_ms = self.authority.cells.last().map_or(0, |cell| cell.range.1);
        Ok(())
    }

    fn analyze(&mut self, final_input: bool) -> Result<(),String> {
        let captured_ms = (self.pcm.len()/32) as u64;
        let mut start = self.authority.cells.last().map_or(0, |cell| cell.range.1);
        while start < captured_ms {
            let end = (start+CELL_MS).min(captured_ms);
            let context_start = start.saturating_sub(CONTEXT_MS);
            let context_end = end+CONTEXT_MS;
            if !final_input && context_end > captured_ms { break; }
            let context_end = context_end.min(captured_ms);
            let Some(speech_ms) = self.quality.resolved_speech_ms(context_start,context_end) else { break; };
            // The canonical VAD's existing minimum speech contract applies to
            // this model input. A score on quiet/unknown audio is not identity.
            let sufficient = speech_ms as f32 >= crate::speech_vad_windows::VAD_MIN_SPEECH_SECONDS * 1000.0;
            let identity = if sufficient {
                let sample = &self.pcm[context_start as usize*32..context_end as usize*32];
                let embedding = self.runtime.lock().embedding(sample)?;
                let score = self.reference.cosine(&embedding);
                log::debug!("[word-identity] source={} generation={} cell={start}..{end} context={context_start}..{context_end} speech_ms={speech_ms} cosine={score:.6}", self.authority.source.stream,self.authority.source.generation);
                if score >= OPERATING_POINT { WordIdentity::Owner } else { WordIdentity::Other }
            } else { WordIdentity::Pending };
            self.authority.cells.push(IdentityCell { range:(start,end),context:(context_start,context_end),identity,
                observed_at:Instant::now(),speech_end_ms:self.quality.confirmed_speech_end_ms_in(start,end) });
            start = end;
        }
        self.authority.settled_through_ms = self.authority.cells.last().map_or(0, |cell|
            cell.range.1);
        Ok(())
    }
}

pub(crate) struct OriginalIdentityStream {
    input: Mutex<Option<mpsc::Sender<Vec<u8>>>>,
    authority: Arc<Mutex<WordAuthority>>,
    done: Arc<AtomicBool>,
    completed: Arc<tokio::sync::Notify>,
    cancelled: Arc<AtomicBool>,
}

impl OriginalIdentityStream {
    pub(crate) fn start(source: AudioSource, reference: EresEmbedding) -> Arc<Self> {
        let (tx,rx) = mpsc::channel::<Vec<u8>>();
        let authority = Arc::new(Mutex::new(WordAuthority::new(source)));
        let done = Arc::new(AtomicBool::new(false));
        let completed = Arc::new(tokio::sync::Notify::new());
        let cancelled = Arc::new(AtomicBool::new(false));
        let result = Arc::new(Self { input:Mutex::new(Some(tx)),authority:Arc::clone(&authority),done:Arc::clone(&done),
            completed:Arc::clone(&completed),cancelled:Arc::clone(&cancelled) });
        tauri::async_runtime::spawn_blocking(move || {
            let analysis = (|| {
                let runtime = crate::speaker_identity_eres::local_runtime()?;
                let mut analyzer = OriginalIdentityAnalyzer::new(source,reference,runtime)?;
                while let Ok(pcm) = rx.recv() {
                    if cancelled.load(Ordering::Acquire) { return Err("original identity analysis cancelled".into()); }
                    analyzer.accept(&pcm)?;
                    *authority.lock() = analyzer.authority.clone();
                }
                if cancelled.load(Ordering::Acquire) { return Err("original identity analysis cancelled".into()); }
                analyzer.finish()?;
                *authority.lock() = analyzer.authority;
                Ok::<_,String>(())
            })();
            if let Err(error) = analysis {
                log::warn!("[word-identity] source={} generation={} unavailable; permission remains pending: {error}",source.stream,source.generation);
                authority.lock().unavailable = Some(error);
            }
            done.store(true,Ordering::Release);
            completed.notify_one();
        });
        result
    }

    pub(crate) fn consume(&self, pcm: &[u8]) {
        if let Some(sender) = self.input.lock().as_ref() {
            if sender.send(pcm.to_vec()).is_err() { self.authority.lock().unavailable.get_or_insert("original identity worker stopped".into()); }
        }
    }
    pub(crate) fn snapshot(&self) -> WordAuthority { self.authority.lock().clone() }
    pub(crate) fn finish_input(&self) { self.input.lock().take(); }
    pub(crate) fn cancel(&self) { self.cancelled.store(true,Ordering::Release); self.finish_input(); }
    pub(crate) async fn finish(&self) -> WordAuthority {
        self.finish_input();
        while !self.done.load(Ordering::Acquire) { self.completed.notified().await; }
        self.snapshot()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires the fixed consented reference, saved recordings and local model/runtime"]
    fn fixed_recordings_require_background_exclusion_and_owner_preservation_together() {
        use crate::transcript_boundary::{TimedTranscript,TimedTranscriptWord};
        let fixture: serde_json::Value = serde_json::from_slice(&std::fs::read(
            std::env::var_os("LISTENER_ERES_FIXTURES").expect("fixtures required")).unwrap()).unwrap();
        let dll = std::env::var_os("LISTENER_ERES_RUNTIME").unwrap();
        let mut runtime = EresRuntime::load(std::path::Path::new(fixture["model_onnx"]["path"].as_str().unwrap()),std::path::Path::new(&dll)).unwrap();
        let reference = runtime.embedding(&std::fs::read(fixture["items"][0]["pcm"]["path"].as_str().unwrap()).unwrap()).unwrap();
        let runtime = Arc::new(Mutex::new(runtime));
        let word_fixture: serde_json::Value = serde_json::from_str(include_str!("word_authority_recorded_fixture.json")).unwrap();
        let mut failures = Vec::new();
        for (id,path) in [("long","long-body.pcm"),("quiet","quiet-body.pcm")] {
            let root = std::path::Path::new(fixture["model_onnx"]["path"].as_str().unwrap()).parent().unwrap().parent().unwrap().parent().unwrap();
            let pcm = std::fs::read(root.join(path)).unwrap();
            use sha2::{Digest,Sha256};
            assert_eq!(format!("{:x}",Sha256::digest(&pcm)),word_fixture[id]["pcm_sha256"].as_str().unwrap());
            let source = AudioSource { stream:1,generation:1 };
            let mut analyzer = OriginalIdentityAnalyzer::new(source,reference.clone(),Arc::clone(&runtime)).unwrap();
            for chunk in pcm.chunks(640) { analyzer.accept(chunk).unwrap(); }
            analyzer.finish().unwrap();
            for sample in word_fixture[id]["samples"].as_array().unwrap() {
                let timing = TimedTranscript { text:sample["text"].as_str().unwrap().into(),
                    words:sample["words"].as_array().unwrap().iter().map(|word| TimedTranscriptWord {
                        text:word["text"].as_str().unwrap().into(),start_ms:word["start_ms"].as_u64(),end_ms:word["end_ms"].as_u64(),
                    }).collect() };
                let projection = analyzer.authority.project(source,&timing,false);
                let decisions = timing.words.iter().map(|word| analyzer.authority.identity(source,word.start_ms.unwrap(),word.end_ms.unwrap())).collect::<Vec<_>>();
                println!("{id} {}: {decisions:?}; projected={}; pending={}",sample["id"],projection.timing.text,projection.unresolved);
                let expected = if sample["owner"].as_bool().unwrap() { timing.text.as_str() } else { "" };
                if projection.timing.text != expected || projection.unresolved { failures.push(format!("{id} {}",sample["id"])); }
            }
        }
        assert!(failures.is_empty(),"recorded identity failures: {failures:?}");
    }
}
