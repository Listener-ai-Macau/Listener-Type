//! Identity and ordered transport for the separator's own PCM.
use super::target_speaker_extraction::ExtractedChunk;
use crate::speech_decision_kernel::SeparatedAudioOwnership;
use crate::speaker_verification::{SessionSpeakerObservation, SessionSpeakerProfile, VerifiedExtractedOutputContext};
use crate::speech_vad_windows::SourceSpeechQuality;
use std::collections::VecDeque;

pub(super) struct ExtractedOutputDecision {
    pub(super) extracted: ExtractedChunk,
    pub(super) ownership: SeparatedAudioOwnership,
    pub(super) silence_confirmed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(range: (u64, u64), ownership: SeparatedAudioOwnership) -> PendingOutput {
        PendingOutput { range, observation: None, decision: ExtractedOutputDecision {
            extracted: ExtractedChunk { pcm: vec![1,2], residual_energy: 1.0,
                mixture_energy: 1.0, target_to_mix_energy: 1.0, target_similarity: Some(0.99) },
            ownership, silence_confirmed: false,
        } }
    }

    #[test]
    fn unresolved_prefix_cannot_be_zeroed_or_overtaken_by_later_owner() {
        use SeparatedAudioOwnership::{Owner, Pending};
        let mut queue = ExtractedOutputQueue { quality: None, context: None,
            pending: VecDeque::from([pending((0,3000), Pending), pending((3000,6000), Owner)]), next_start_ms: 6000 };
        assert!(queue.drain_ready().is_empty());
        assert!(queue.unresolved());
        queue.pending.front_mut().unwrap().decision.ownership = Owner;
        let ready = queue.drain_ready();
        assert_eq!(ready.iter().map(|(_,range)| *range).collect::<Vec<_>>(), [(0,3000),(3000,6000)]);
        assert!(ready.iter().all(|(decision,_)| decision.owner_transport_pcm() == Some(vec![1,2])));
        assert!(!queue.unresolved());
    }
}

impl std::ops::Deref for ExtractedOutputDecision {
    type Target = ExtractedChunk;
    fn deref(&self) -> &Self::Target { &self.extracted }
}

impl ExtractedOutputDecision {
    fn is_resolved(&self) -> bool { self.ownership != SeparatedAudioOwnership::Pending || self.silence_confirmed }
    pub(super) fn ownership(&self) -> SeparatedAudioOwnership { self.ownership }
    pub(super) fn owner_transport_pcm(&self) -> Option<Vec<u8>> {
        use SeparatedAudioOwnership::{Owner, Other, Pending};
        match self.ownership {
            Owner => Some(self.pcm.clone()),
            Other => Some(vec![0; self.pcm.len()]),
            Pending if self.silence_confirmed => Some(vec![0; self.pcm.len()]),
            Pending => None,
        }
    }
    pub(super) fn supports_physical_interference_measurement(&self) -> bool {
        self.ownership == SeparatedAudioOwnership::Owner
    }
    pub(super) fn confidently_non_target(&self) -> bool {
        self.ownership == SeparatedAudioOwnership::Other
    }
}

struct PendingOutput {
    decision: ExtractedOutputDecision,
    range: (u64, u64),
    observation: Option<SessionSpeakerObservation>,
}

/// Pending audio is retained in order. No permanent zero packet is emitted
/// for unknown identity. A later BASE-qualified output observation may supply
/// bounded context; context matches never enroll further references.
pub(super) struct ExtractedOutputQueue {
    quality: Option<SourceSpeechQuality>,
    context: Option<VerifiedExtractedOutputContext>,
    pending: VecDeque<PendingOutput>,
    next_start_ms: u64,
}

impl ExtractedOutputQueue {
    pub(super) fn new() -> Self {
        let quality = SourceSpeechQuality::new().map_err(|error| {
            log::warn!("[target-speaker] extracted quality unavailable; identity remains pending: {error}");
        }).ok();
        Self { quality, context: None, pending: VecDeque::new(), next_start_ms: 0 }
    }

    pub(super) fn submit(&mut self, extracted: ExtractedChunk, range: (u64, u64),
        profile: Option<SessionSpeakerProfile>,
    ) -> Result<Vec<(ExtractedOutputDecision, (u64, u64))>, String> {
        if range.0 != self.next_start_ms { return Err("noncontiguous extracted output".into()); }
        self.next_start_ms = range.1;
        // At most one minute of unresolved PCM (1.92 MB). Exhaustion is an
        // explicit incomplete analysis, never silence or a clean certificate.
        if self.pending.iter().map(|chunk| chunk.decision.pcm.len()).sum::<usize>() + extracted.pcm.len() > 60 * 32000 {
            return Err("extracted output identity unresolved beyond buffer bound".into());
        }
        if let Some(quality) = self.quality.as_mut() { quality.accept(&extracted.pcm)?; }
        if self.context.is_none() { self.context = profile.and_then(VerifiedExtractedOutputContext::new); }
        self.pending.push_back(PendingOutput { decision: ExtractedOutputDecision {
            extracted, ownership: SeparatedAudioOwnership::Pending, silence_confirmed: false,
        }, range, observation: None });
        self.resolve();
        Ok(self.drain_ready())
    }

    pub(super) fn finish(&mut self, profile: Option<SessionSpeakerProfile>) -> Vec<(ExtractedOutputDecision, (u64, u64))> {
        if let Some(quality) = self.quality.as_mut() { quality.finish(); }
        if self.context.is_none() { self.context = profile.and_then(VerifiedExtractedOutputContext::new); }
        self.resolve();
        #[cfg(test)]
        if let Some(directory) = std::env::var_os("LISTENER_TEST_SEPARATOR_PCM_EXPORT_DIR") {
            let rows = self.pending.iter().map(|chunk| serde_json::json!({
                "range": chunk.range,
                "ownership": format!("{:?}", chunk.decision.ownership),
                "silence_confirmed": chunk.decision.silence_confirmed,
                "quality_available": self.quality.as_ref().is_some_and(SourceSpeechQuality::is_valid),
                "speech_upper_bound_ms": self.quality.as_ref().and_then(|quality| quality.speech_upper_bound_ms(chunk.range.0, chunk.range.1)),
                "classification": chunk.observation.as_ref().map(|observation| format!("{:?}", observation.classification)),
                "energy_speech_ms": chunk.observation.as_ref().map(|observation| observation.real_speech_ms),
            })).collect::<Vec<_>>();
            if let Err(error) = std::fs::write(std::path::PathBuf::from(directory).join("output-drain-decisions.json"),
                serde_json::to_vec_pretty(&rows).expect("diagnostic values serialize")) {
                log::warn!("[target-speaker] cannot export output drain diagnostics: {error}");
            }
        }
        self.drain_ready()
    }

    pub(super) fn unresolved(&self) -> bool { !self.pending.is_empty() }

    fn resolve(&mut self) {
        let quality_available = self.quality.as_ref().is_some_and(SourceSpeechQuality::is_valid);
        // Second pass reconsiders earlier pending windows after this pass has
        // admitted a later independently qualified reference.
        for _ in 0..2 {
            for chunk in &mut self.pending {
                if chunk.decision.is_resolved() { continue; }
                let bound = self.quality.as_ref().and_then(|quality| quality.speech_upper_bound_ms(chunk.range.0, chunk.range.1));
                if quality_available && bound == Some(0) {
                    chunk.decision.silence_confirmed = true;
                    continue;
                }
                if let Some(context) = self.context.as_mut() {
                    if chunk.observation.is_none() {
                        chunk.observation = context.observe(&chunk.decision.pcm).ok();
                    }
                    if let Some(observation) = chunk.observation.as_ref() {
                        chunk.decision.ownership = context.classify(observation, bound, quality_available);
                    }
                }
            }
        }
    }

    fn drain_ready(&mut self) -> Vec<(ExtractedOutputDecision, (u64, u64))> {
        let mut ready = Vec::new();
        while self.pending.front().is_some_and(|chunk| chunk.decision.is_resolved()) {
            let chunk = self.pending.pop_front().unwrap();
            ready.push((chunk.decision, chunk.range));
        }
        ready
    }
}
