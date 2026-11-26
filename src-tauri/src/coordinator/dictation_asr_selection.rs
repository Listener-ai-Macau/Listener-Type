fn dictation_asr_engine_backend_id(active_asr: &str) -> &'static str {
    let active_asr = crate::asr::local::companion_paraformer::canonical_provider(active_asr);
    if crate::asr::local::is_local_qwen3(active_asr) {
        "local-qwen3"
    } else if crate::asr::local::companion_paraformer::is_provider(active_asr) {
        crate::asr::local::companion_paraformer::PROVIDER_ID
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
        crate::asr::local::companion_paraformer::PROVIDER_ID => "Companion Paraformer Large".to_string(),
        "bailian" => "Bailian".to_string(),
        "whisper-compatible" => format!("Whisper-compatible ({})", active_asr.trim()),
        _ => active_asr.trim().to_string(),
    }
}

fn dictation_asr_uses_core_accurate_engine(active_asr: &str) -> bool {
    dictation_asr_engine_backend_id(active_asr) == "volcengine"
}

fn dictation_asr_quality_warning(active_asr: &str) -> Option<String> {
    if dictation_asr_uses_core_accurate_engine(active_asr)
        || is_windows_local_asr_provider(active_asr)
    {
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
    if !core_accurate && !is_windows_local_asr_provider(active_asr) {
        log::warn!(
            "[coord] dictation ASR non-core accuracy warning: session_id={session_id} provider={} backend={backend}",
            active_asr.trim()
        );
    }
}
