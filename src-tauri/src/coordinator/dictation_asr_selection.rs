fn foundry_language_hint_from_preferences(prefs: &UserPreferences) -> FoundryLanguageHintSelection {
    let explicit = prefs.foundry_local_asr_language_hint.trim();
    // Recognition language is an input setting. Output translation, Chinese
    // script and working-language preferences must not force the decoder when
    // the user selected automatic recognition.
    FoundryLanguageHintSelection {
        hint: if explicit.is_empty() { None } else { Some(explicit.to_string()) },
        source: if explicit.is_empty() { "auto" } else { "explicit" },
    }
}

#[cfg(test)]
mod foundry_language_selection_tests {
    use super::foundry_language_hint_from_preferences;
    use crate::types::{ChineseScriptPreference, OutputLanguagePreference, UserPreferences};

    #[test]
    fn automatic_recognition_keeps_input_language_independent_of_output_preferences() {
        for output in [OutputLanguagePreference::Auto, OutputLanguagePreference::ZhCn,
            OutputLanguagePreference::ZhTw, OutputLanguagePreference::En,
            OutputLanguagePreference::Ja, OutputLanguagePreference::Ko] {
            for script in [ChineseScriptPreference::Auto, ChineseScriptPreference::Simplified,
                ChineseScriptPreference::Traditional] {
                for hint in ["", "   "] {
                    let mut prefs = UserPreferences::default();
                    prefs.foundry_local_asr_language_hint = hint.into();
                    prefs.output_language_preference = output;
                    prefs.chinese_script_preference = script;
                    prefs.working_languages = vec!["中文".into(), "en".into(), "日本語".into()];
                    let selected = foundry_language_hint_from_preferences(&prefs);
                    assert_eq!(selected.hint, None, "output={output:?} script={script:?}");
                    assert_eq!(selected.source, "auto");
                }
            }
        }
    }

    #[test]
    fn explicit_recognition_language_survives_conflicting_output_preferences() {
        for language in ["zh", "en", "ja", "ko"] {
            let mut prefs = UserPreferences::default();
            prefs.foundry_local_asr_language_hint = format!(" {language} ");
            prefs.output_language_preference = OutputLanguagePreference::ZhCn;
            prefs.chinese_script_preference = ChineseScriptPreference::Traditional;
            prefs.working_languages = vec!["中文".into()];
            let selected = foundry_language_hint_from_preferences(&prefs);
            assert_eq!(selected.hint.as_deref(), Some(language));
            assert_eq!(selected.source, "explicit");
        }
    }
}

fn dictation_asr_engine_backend_id(active_asr: &str) -> &'static str {
    if crate::asr::local::is_local_qwen3(active_asr) {
        "local-qwen3"
    } else if crate::asr::local::foundry::is_foundry_local_whisper(active_asr) {
        "foundry-local-whisper"
    } else if is_bailian_provider(active_asr) {
        "bailian"
    } else if is_whisper_compatible_provider(active_asr) {
        "whisper-compatible"
    } else {
        "volcengine"
    }
}

fn dictation_asr_engine_label(active_asr: &str) -> String {
    match dictation_asr_engine_backend_id(active_asr) {
        "volcengine" => "Volcengine".to_string(),
        "local-qwen3" => "Local Qwen3-ASR".to_string(),
        "foundry-local-whisper" => "Foundry Local Whisper".to_string(),
        "bailian" => "Bailian".to_string(),
        "whisper-compatible" => format!("Whisper-compatible ({})", active_asr.trim()),
        _ => active_asr.trim().to_string(),
    }
}

fn dictation_asr_uses_core_accurate_engine(active_asr: &str) -> bool {
    dictation_asr_engine_backend_id(active_asr) == "volcengine"
}

fn dictation_asr_quality_warning(active_asr: &str) -> Option<String> {
    if dictation_asr_uses_core_accurate_engine(active_asr) {
        return None;
    }
    Some(format!(
        "当前识别引擎为{}，不是核心 Volcengine 准确引擎，识别可能不准。",
        dictation_asr_engine_label(active_asr)
    ))
}

fn log_dictation_asr_engine_selection(session_id: SessionId, active_asr: &str) {
    let backend = dictation_asr_engine_backend_id(active_asr);
    let label = dictation_asr_engine_label(active_asr);
    let core_accurate = dictation_asr_uses_core_accurate_engine(active_asr);
    log::info!(
        "[coord] dictation ASR engine selected: session_id={session_id} provider={} backend={backend} label=\"{label}\" core_accurate={core_accurate}",
        active_asr.trim()
    );
    if !core_accurate {
        log::warn!(
            "[coord] dictation ASR non-core accuracy warning: session_id={session_id} provider={} backend={backend}",
            active_asr.trim()
        );
    }
}
