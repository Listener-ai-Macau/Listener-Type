//! One existing wake model, independent keyword streams. Extra-command setup
//! failure cannot remove the previously configured dictation wake detector.
use crate::voice_command::{VoiceAction, VoiceCommandLexicon};
use crate::wake_phrase::{Match, StreamingDetector};

#[derive(Debug, Clone)]
pub(crate) struct CommandKeywordMatch {
    pub action: VoiceAction,
    pub phrase: String,
    pub keyword: Match,
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;
    #[test]
    #[ignore = "explicit saved/synthetic WAV manifest and verified existing KWS cache only; no microphone/cloud/editor"]
    fn saved_commands_shadow_primary_and_dual_decoders() {
        use sha2::{Digest, Sha256};
        let manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(
            std::env::var("LISTENER_COMMAND_SHADOW_MANIFEST").unwrap()).unwrap()).unwrap();
        let mut rows = Vec::new();
        for clip in manifest["clips"].as_array().unwrap() {
            let wav = std::fs::read(clip["wav"].as_str().unwrap()).unwrap();
            let pcm = denzic_audio_v1_core::read_wav_pcm16le(&wav).unwrap();
            for batch in [1600, 6400] {
                let mut primary = VoiceCommandDetector::new("开始录音", None).unwrap();
                let mut dual = VoiceCommandDetector::new("开始录音", Some("开始提问")).unwrap();
                assert!(dual.question_unavailable.is_none(), "{:?}", dual.question_unavailable);
                for chunk in pcm.chunks(batch) {
                    primary.accept_pcm(chunk).unwrap(); dual.accept_pcm(chunk).unwrap();
                }
                primary.finish().unwrap(); dual.finish().unwrap();
                let encode = |detector: &VoiceCommandDetector| detector.selected.as_ref().map(|found|
                    serde_json::json!({"action":format!("{:?}",found.action),
                        "label":found.keyword.matched_keyword,"end":found.keyword.audio_end_seconds}));
                let row = serde_json::json!({"id":clip["id"],"batch":batch,
                    "pcmSha256":format!("{:x}",Sha256::digest(&pcm)),
                    "primary":encode(&primary),"dual":encode(&dual)});
                println!("command_shadow {row}"); rows.push(row);
            }
        }
        std::fs::write(std::env::var("LISTENER_COMMAND_SHADOW_OUTPUT").unwrap(),
            serde_json::to_vec_pretty(&serde_json::json!({"rows":rows,
                "scope":"Decoder evidence only, not actual phrase/identity admission or current-event reproduction"})).unwrap()).unwrap();
    }
    #[test]
    #[ignore = "requires explicitly generated local speech WAVs and the existing KWS cache"]
    fn existing_model_separates_record_question_and_ordinary_speech() {
        let root = std::path::PathBuf::from(std::env::var("LISTENER_COMMAND_WAV_ROOT").unwrap());
        for (name, expected) in [("dictation", Some(VoiceAction::Dictation)),
            ("question", Some(VoiceAction::Question)), ("ordinary", None)] {
            let wav = std::fs::read(root.join(format!("{name}.wav"))).unwrap();
            assert_eq!(&wav[..4], b"RIFF");
            assert_eq!(&wav[8..12], b"WAVE");
            let mut offset = 12;
            let mut pcm = None;
            while offset + 8 <= wav.len() {
                let bytes = u32::from_le_bytes(wav[offset+4..offset+8].try_into().unwrap()) as usize;
                let data = wav.get(offset+8..offset+8+bytes).expect("complete WAV chunk");
                if &wav[offset..offset+4] == b"fmt " {
                    assert_eq!(u16::from_le_bytes(data[..2].try_into().unwrap()), 1);
                    assert_eq!(u16::from_le_bytes(data[2..4].try_into().unwrap()), 1);
                    assert_eq!(u32::from_le_bytes(data[4..8].try_into().unwrap()), 16000);
                    assert_eq!(u16::from_le_bytes(data[14..16].try_into().unwrap()), 16);
                }
                if &wav[offset..offset+4] == b"data" { pcm = Some(data.to_vec()); }
                offset += 8 + bytes + bytes % 2;
            }
            let pcm = pcm.expect("PCM data chunk");
            let mut detector = VoiceCommandDetector::new("开始录音", Some("开始提问")).unwrap();
            assert!(detector.question_unavailable.is_none(), "{:?}", detector.question_unavailable);
            for chunk in pcm.chunks(6400) { detector.accept_pcm(chunk).unwrap(); }
            detector.finish().unwrap();
            let found = detector.selected.as_ref().map(|found| found.action);
            println!("fixture={name} expected={expected:?} actual={:?}", detector.selected);
            assert_eq!(found, expected, "{name}");
        }
    }
}

pub(crate) struct VoiceCommandDetector {
    primary: StreamingDetector,
    question: Option<StreamingDetector>,
    dictation_phrase: String,
    lexicon: Option<VoiceCommandLexicon>,
    pub question_unavailable: Option<String>,
    pub selected: Option<CommandKeywordMatch>,
}

impl VoiceCommandDetector {
    pub(crate) fn new(dictation: &str, question: Option<&str>) -> Result<Self, String> {
        let dictation_phrase = crate::wake_phrase::normalize_configured_phrase(dictation)?;
        let primary = StreamingDetector::new(&dictation_phrase)?;
        let mut detector = Self { primary, question: None, dictation_phrase,
            lexicon: None, question_unavailable: None, selected: None };
        if let Some(question) = question {
            let setup = crate::wake_phrase::normalize_configured_phrase(question)
                .and_then(|question| VoiceCommandLexicon::from_normalized(
                    detector.dictation_phrase.clone(), question))
                .and_then(|lexicon| detector.primary.fork_strict_for_phrase(
                    lexicon.phrase(VoiceAction::Question)).map(|stream| (lexicon, stream)));
            match setup {
                Ok((lexicon, stream)) => { detector.lexicon = Some(lexicon); detector.question = Some(stream); }
                Err(error) => detector.question_unavailable = Some(error),
            }
        }
        Ok(detector)
    }

    pub(crate) fn accept_pcm(&mut self, pcm: &[u8]) -> Result<Option<Match>, String> {
        // Continue the old primary/complete stream even after a recall hit;
        // stopping it early would lose its later complete-keyword boundary.
        let primary = self.primary.accept_pcm(pcm)?;
        let question = match self.question.as_mut() {
            Some(stream) => stream.accept_pcm(pcm)?,
            None => None,
        };
        let selected = self.choose(primary, question);
        if let Some(found) = selected { self.selected = Some(found); }
        Ok(self.selected.as_ref().map(|found| found.keyword.clone()))
    }

    pub(crate) fn finish(&mut self) -> Result<Option<Match>, String> {
        let primary = self.primary.finish()?;
        let question = match self.question.as_mut() {
            Some(stream) => stream.finish()?,
            None => None,
        };
        let selected = self.choose(primary, question);
        if let Some(found) = selected { self.selected = Some(found); }
        Ok(self.selected.as_ref().map(|found| found.keyword.clone()))
    }

    fn choose(&self, primary: Option<Match>, question: Option<Match>) -> Option<CommandKeywordMatch> {
        let question = question.filter(|found| self.lexicon.as_ref().is_some_and(|words|
            found.matched_keyword.as_deref().and_then(|label|
                words.classify_verified_full_keyword(label)) == Some(VoiceAction::Question)));
        let question_first = match (&primary, &question) {
            (Some(primary), Some(question)) => {
                let primary_complete = primary.matched_keyword.as_deref() == Some(self.dictation_phrase.as_str());
                !primary_complete || question.end_seconds < primary.end_seconds
            }
            (None, Some(_)) => true,
            _ => false,
        };
        if question_first {
            question.map(|keyword| CommandKeywordMatch { action: VoiceAction::Question,
                phrase: self.lexicon.as_ref().expect("validated question lexicon").phrase(VoiceAction::Question).into(), keyword })
        } else {
            primary.map(|keyword| CommandKeywordMatch { action: VoiceAction::Dictation,
                phrase: self.dictation_phrase.clone(), keyword })
        }
    }
}
