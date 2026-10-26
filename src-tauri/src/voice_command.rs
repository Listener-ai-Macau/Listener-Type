//! Voice intent is selected from a verified keyword, before a capture may
//! deliver text. Never infer a question mode from already-committed body text.

pub(crate) const DEFAULT_QUESTION_PHRASE: &str = "开始提问";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VoiceAction {
    Dictation,
    Question,
}

impl VoiceAction {
    pub(crate) fn permits_editor_delivery(self) -> bool {
        self == Self::Dictation
    }
}

#[derive(Debug, Clone)]
pub(crate) struct VoiceCommandLexicon {
    dictation: String,
    question: String,
}

impl VoiceCommandLexicon {
    /// Call the existing wake-phrase normalizer first. A conflicting optional
    /// question phrase must be rejected, never replace the dictation phrase.
    pub(crate) fn from_normalized(dictation: String, question: String) -> Result<Self, String> {
        if dictation.is_empty() || question.is_empty()
            || dictation.chars().any(char::is_whitespace)
            || question.chars().any(char::is_whitespace)
        {
            return Err("voice command phrases must be normalized and nonempty".into());
        }
        if dictation.starts_with(&question) || question.starts_with(&dictation) {
            return Err("question and dictation wake phrases must be distinct and nonoverlapping".into());
        }
        Ok(Self { dictation, question })
    }

    pub(crate) fn phrase(&self, action: VoiceAction) -> &str {
        match action {
            VoiceAction::Dictation => &self.dictation,
            VoiceAction::Question => &self.question,
        }
    }

    pub(crate) fn classify_verified_full_keyword(&self, label: &str) -> Option<VoiceAction> {
        if label == self.dictation {
            Some(VoiceAction::Dictation)
        } else if label == self.question {
            Some(VoiceAction::Question)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lexicon() -> VoiceCommandLexicon {
        VoiceCommandLexicon::from_normalized("开始录音".into(), DEFAULT_QUESTION_PHRASE.into()).unwrap()
    }

    #[test]
    fn question_is_separate_and_cannot_deliver_into_the_editor() {
        let words = lexicon();
        assert_eq!(words.classify_verified_full_keyword("开始录音"), Some(VoiceAction::Dictation));
        let question = words.classify_verified_full_keyword("开始提问").unwrap();
        assert_eq!(question, VoiceAction::Question);
        assert!(!question.permits_editor_delivery());
        assert!(VoiceAction::Dictation.permits_editor_delivery());
        assert_eq!(words.phrase(VoiceAction::Dictation), "开始录音");
    }

    #[test]
    fn body_text_and_incomplete_or_similar_keywords_do_not_select_a_route() {
        for text in ["开始提", "始提问", "开始提文", "请开始提问", "正文里说开始提问", "开始录音开始提问"] {
            assert_eq!(lexicon().classify_verified_full_keyword(text), None, "{text}");
        }
    }

    #[test]
    fn conflict_is_rejected_without_rewriting_a_custom_dictation_phrase() {
        let original = "开始提问".to_owned();
        assert!(VoiceCommandLexicon::from_normalized(original.clone(), DEFAULT_QUESTION_PHRASE.into()).is_err());
        assert_eq!(original, "开始提问");
        assert!(VoiceCommandLexicon::from_normalized("开始".into(), DEFAULT_QUESTION_PHRASE.into()).is_err());
    }
}
