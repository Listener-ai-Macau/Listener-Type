fn foundry_language_hint_from_preferences(prefs: &UserPreferences) -> FoundryLanguageHintSelection {
    let explicit = prefs.foundry_local_asr_language_hint.trim();
    if !explicit.is_empty() {
        return FoundryLanguageHintSelection {
            hint: Some(explicit.to_string()),
            source: "explicit",
        };
    }

    if let Some(hint) = foundry_language_hint_for_output_language(prefs.output_language_preference)
    {
        return FoundryLanguageHintSelection {
            hint: Some(hint.to_string()),
            source: "output_language_preference",
        };
    }

    if matches!(
        prefs.chinese_script_preference,
        ChineseScriptPreference::Simplified | ChineseScriptPreference::Traditional
    ) {
        return FoundryLanguageHintSelection {
            hint: Some("zh".to_string()),
            source: "chinese_script_preference",
        };
    }

    for language in &prefs.working_languages {
        if let Some(hint) = foundry_language_hint_for_working_language(language) {
            return FoundryLanguageHintSelection {
                hint: Some(hint.to_string()),
                source: "working_languages",
            };
        }
    }

    FoundryLanguageHintSelection {
        hint: None,
        source: "auto",
    }
}

fn foundry_language_hint_for_output_language(
    preference: OutputLanguagePreference,
) -> Option<&'static str> {
    match preference {
        OutputLanguagePreference::ZhCn | OutputLanguagePreference::ZhTw => Some("zh"),
        OutputLanguagePreference::En => Some("en"),
        OutputLanguagePreference::Ja => Some("ja"),
        OutputLanguagePreference::Ko => Some("ko"),
        OutputLanguagePreference::Auto => None,
    }
}

fn foundry_language_hint_for_working_language(language: &str) -> Option<&'static str> {
    let normalized = language.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return None;
    }

    if language.contains("中文")
        || language.contains("汉语")
        || language.contains("漢語")
        || language.contains("简体")
        || language.contains("簡體")
        || language.contains("繁体")
        || language.contains("繁體")
        || normalized == "zh"
        || normalized.starts_with("zh-")
        || normalized.contains("chinese")
    {
        return Some("zh");
    }

    if normalized == "en" || normalized.starts_with("en-") || normalized.contains("english") {
        return Some("en");
    }

    if language.contains("日本")
        || language.contains("日语")
        || language.contains("日語")
        || normalized == "ja"
        || normalized.starts_with("ja-")
        || normalized.contains("japanese")
    {
        return Some("ja");
    }

    if language.contains("한국")
        || language.contains("韩语")
        || language.contains("韓語")
        || normalized == "ko"
        || normalized.starts_with("ko-")
        || normalized.contains("korean")
    {
        return Some("ko");
    }

    None
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
