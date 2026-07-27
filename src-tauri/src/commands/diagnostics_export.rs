// Diagnostic package / error-log export commands.
// Included into `commands` via `include!`.

/// 把当前会话的 listener-type.log 复制到用户选择的位置（前端用 plugin-dialog 拿 target_path）。
#[tauri::command]
pub fn export_error_log(target_path: String) -> Result<(), String> {
    let src = crate::log_dir_path().join("listener-type.log");
    if !src.exists() {
        return Err(format!("日志文件不存在：{}", src.display()));
    }
    std::fs::copy(&src, std::path::Path::new(&target_path))
        .map(|_| ())
        .map_err(|e| format!("复制日志失败：{}", e))
}

/// Export the minimal first-start/OOBE diagnostic package used by the Windows installer path.
///
/// The package is intentionally metadata-only: it excludes audio bytes, raw transcripts,
/// final inserted text and credential values. Credentials are represented only as
/// configured/unconfigured booleans.
#[tauri::command]
pub fn export_diagnostic_package(
    coord: CoordinatorState<'_>,
    target_path: String,
) -> Result<String, String> {
    let package = build_diagnostic_package(coord.inner())?;
    let firmware_log = crate::embedded_ble::pull_firmware_diagnostic_log(Duration::from_secs(45));
    let target = diagnostic_export_target_path(&target_path, &package)?;
    write_diagnostic_package_zip(&target, &package, &firmware_log)?;
    Ok(target.display().to_string())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticPackage {
    schema_version: u32,
    generated_at: String,
    app: DiagnosticApp,
    platform: DiagnosticPlatform,
    firmware: DiagnosticFirmware,
    ble: DiagnosticBle,
    config: DiagnosticConfig,
    credentials: DiagnosticCredentials,
    recent_errors: Vec<String>,
    timeline: Vec<String>,
    recent_sessions: Vec<DiagnosticRecentSession>,
    privacy: DiagnosticPrivacy,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticApp {
    product_name: &'static str,
    identifier: &'static str,
    version: &'static str,
    log_path: String,
    executable_path: Option<String>,
    windows_ime_status: WindowsImeStatus,
    hotkey_status: HotkeyStatus,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticPlatform {
    os: &'static str,
    family: &'static str,
    arch: &'static str,
    debug_build: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticFirmware {
    device_model: &'static str,
    version: Option<String>,
    protocol: &'static str,
    protocol_version: Option<u32>,
    readiness: Option<Value>,
    wake_policy: FirmwareWakePolicySnapshot,
    source: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticBle {
    input_source: Value,
    enabled_by_input_source: bool,
    background_listener_disabled_by_env: bool,
    background_listener_active: bool,
    background_listener_ready: bool,
    background_listener_generation: u64,
    service_uuid: &'static str,
    diagnostic_snapshot: crate::embedded_ble::BleDiagnosticSnapshot,
    failure_taxonomy: Vec<crate::embedded_ble::BleFailureClassification>,
    device_address: Option<String>,
    firmware_version: Option<String>,
    battery_percent: Option<u8>,
    capabilities: Vec<String>,
    recent_disconnect_reason: Option<String>,
    reconnect_attempts: u32,
    notify_subscription_state: String,
    wake_recovery: EmbeddedBleWakeRecoverySnapshot,
    session_actor_history: Vec<EmbeddedBleSessionActorDiagnosticRecord>,
    recent_embedded_session_count: usize,
    latest_session_id: Option<String>,
    last_error_code: Option<String>,
    last_embedded_audio_end_reason: Option<Value>,
    last_embedded_audio_stats: Option<crate::embedded_audio::SessionStats>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticConfig {
    dictation_input_source: Value,
    active_asr_provider: String,
    active_llm_provider: String,
    asr_configured: bool,
    llm_configured: bool,
    default_mode: Value,
    active_style_pack_id: String,
    enabled_modes: Value,
    show_capsule: bool,
    start_minimized: bool,
    launch_at_login: bool,
    auto_update_check: bool,
    update_channel: Value,
    history_retention_days: u32,
    history_max_entries: Option<u32>,
    record_audio_for_debug: bool,
    audio_recording_max_entries: Option<u32>,
    local_asr_active_model: String,
    local_asr_keep_loaded_secs: u32,
    foundry_local_asr_model: String,
    foundry_local_runtime_source: String,
    foundry_local_asr_language_hint_configured: bool,
    foundry_local_asr_keep_loaded_secs: u32,
    microphone_device_configured: bool,
    marketplace_backend_configured: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticCredentials {
    active_asr_provider: String,
    active_llm_provider: String,
    asr_configured: bool,
    llm_configured: bool,
    volcengine_configured: bool,
    asr_api_key_configured: bool,
    asr_endpoint_configured: bool,
    asr_model_configured: bool,
    llm_api_key_configured: bool,
    llm_endpoint_configured: bool,
    llm_model_configured: bool,
    codex_oauth_configured: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticRecentSession {
    id: String,
    created_at: String,
    mode: Value,
    insert_status: Value,
    error_code: Option<String>,
    duration_ms: Option<u64>,
    dictionary_entry_count: Option<u32>,
    has_audio_recording: Option<bool>,
    embedded_audio_stats: Option<crate::embedded_audio::SessionStats>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticPrivacy {
    excludes_audio_contents: bool,
    excludes_audio_recordings: bool,
    excludes_raw_transcripts: bool,
    excludes_final_text: bool,
    excludes_api_key_values: bool,
    credential_values_exported: bool,
    notes: Vec<&'static str>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticExportManifest {
    schema_version: u32,
    generated_at: String,
    package_file_name: String,
    app_version: &'static str,
    device_descriptor: String,
    desktop_package_schema_version: u32,
    firmware_diagnostic_log: crate::embedded_ble::FirmwareDiagnosticLogPull,
    files: Vec<DiagnosticZipEntry>,
    privacy: DiagnosticExportPrivacy,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticZipEntry {
    path: String,
    kind: &'static str,
    bytes: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticExportPrivacy {
    audio_contents_included: bool,
    audio_recordings_included: bool,
    raw_transcripts_included: bool,
    final_text_included: bool,
    credential_values_included: bool,
    notes: Vec<&'static str>,
}

struct DiagnosticAudioSampleFile {
    session_id: String,
    zip_path: String,
    bytes: Vec<u8>,
}

const DIAGNOSTIC_AUDIO_SAMPLE_LIMIT: usize = 3;
const DIAGNOSTIC_AUDIO_SAMPLE_MAX_BYTES: u64 = 20 * 1024 * 1024;

fn diagnostic_export_target_path(
    requested_path: &str,
    package: &DiagnosticPackage,
) -> Result<PathBuf, String> {
    let requested = Path::new(requested_path);
    let parent = requested
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let requested_name = requested
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    let file_name = if diagnostic_requested_file_name_is_generic(requested_name) {
        diagnostic_package_file_name(package)
    } else if requested
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
    {
        requested_name.to_string()
    } else {
        format!("{requested_name}.zip")
    };
    Ok(parent.join(file_name))
}

fn diagnostic_requested_file_name_is_generic(name: &str) -> bool {
    if name.trim().is_empty() {
        return true;
    }
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".json")
        || lower.starts_with("listener-type-diagnostic")
        || lower.starts_with("listener-type-ble-wake-diagnostics")
        || lower.starts_with("listener-type-ota-diagnostics")
}

fn diagnostic_package_file_name(package: &DiagnosticPackage) -> String {
    let timestamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    format!(
        "listener-type-diagnostic-{}-{timestamp}.zip",
        diagnostic_device_descriptor(package)
    )
}

fn diagnostic_device_descriptor(package: &DiagnosticPackage) -> String {
    let firmware = package
        .ble
        .firmware_version
        .as_deref()
        .or(package.firmware.version.as_deref())
        .unwrap_or("fw-unknown");
    let address = package
        .ble
        .device_address
        .as_deref()
        .unwrap_or("device-offline");
    sanitize_diagnostic_file_segment(&format!("{firmware}-{address}"))
}

fn sanitize_diagnostic_file_segment(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut previous_dash = false;
    for ch in value.chars() {
        let normalized = if ch.is_ascii_alphanumeric() {
            ch.to_ascii_lowercase()
        } else if matches!(ch, '.' | '_' | '-') {
            ch
        } else {
            '-'
        };
        if normalized == '-' {
            if previous_dash {
                continue;
            }
            previous_dash = true;
        } else {
            previous_dash = false;
        }
        output.push(normalized);
    }
    let trimmed = output.trim_matches('-');
    if trimmed.is_empty() {
        "device-unknown".to_string()
    } else {
        trimmed.chars().take(80).collect()
    }
}

fn write_diagnostic_package_zip(
    target: &Path,
    package: &DiagnosticPackage,
    firmware_log: &crate::embedded_ble::FirmwareDiagnosticLogPull,
) -> Result<(), String> {
    if let Some(parent) = target.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| format!("创建目标目录失败：{e}"))?;
        }
    }

    let log_lines = read_diagnostic_log_tail(1000);
    let desktop_package_bytes =
        serde_json::to_vec_pretty(package).map_err(|e| format!("生成桌面诊断 JSON 失败：{e}"))?;
    let log_tail = sanitized_diagnostic_log_tail(&log_lines);
    let log_tail_bytes = log_tail.as_bytes().to_vec();
    let ble_history_bytes = serde_json::to_vec_pretty(&diagnostic_ble_history(package, &log_lines))
        .map_err(|e| format!("生成 BLE 连接历史失败：{e}"))?;
    let audio_samples = diagnostic_audio_sample_files(package);
    let audio_manifest_bytes =
        serde_json::to_vec_pretty(&diagnostic_audio_samples_manifest(package, &audio_samples))
            .map_err(|e| format!("生成音频样本清单失败：{e}"))?;
    let firmware_summary_bytes = serde_json::to_vec_pretty(firmware_log)
        .map_err(|e| format!("生成固件诊断摘要失败：{e}"))?;

    let mut files = vec![
        DiagnosticZipEntry {
            path: "desktop/diagnostic_package.json".to_string(),
            kind: "desktop_diagnostic_json",
            bytes: desktop_package_bytes.len(),
        },
        DiagnosticZipEntry {
            path: "desktop/listener-type-log-tail.txt".to_string(),
            kind: "desktop_log_tail",
            bytes: log_tail_bytes.len(),
        },
        DiagnosticZipEntry {
            path: "desktop/ble_connection_history.json".to_string(),
            kind: "ble_connection_history",
            bytes: ble_history_bytes.len(),
        },
        DiagnosticZipEntry {
            path: "desktop/audio_samples_manifest.json".to_string(),
            kind: "audio_sample_metadata",
            bytes: audio_manifest_bytes.len(),
        },
        DiagnosticZipEntry {
            path: "firmware/diag_log_summary.json".to_string(),
            kind: "firmware_diagnostic_summary",
            bytes: firmware_summary_bytes.len(),
        },
    ];
    if firmware_log.status == "ok" {
        files.push(DiagnosticZipEntry {
            path: "firmware/diag_log.bin".to_string(),
            kind: "firmware_diagnostic_events",
            bytes: firmware_log.raw_event_bytes.len(),
        });
    }
    for sample in &audio_samples {
        files.push(DiagnosticZipEntry {
            path: sample.zip_path.clone(),
            kind: "audio_sample_wav",
            bytes: sample.bytes.len(),
        });
    }

    let package_file_name = target
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("listener-type-diagnostic.zip")
        .to_string();
    let manifest = DiagnosticExportManifest {
        schema_version: 1,
        generated_at: chrono::Utc::now().to_rfc3339(),
        package_file_name,
        app_version: env!("CARGO_PKG_VERSION"),
        device_descriptor: diagnostic_device_descriptor(package),
        desktop_package_schema_version: package.schema_version,
        firmware_diagnostic_log: firmware_log.clone(),
        files,
        privacy: DiagnosticExportPrivacy {
            audio_contents_included: !audio_samples.is_empty(),
            audio_recordings_included: !audio_samples.is_empty(),
            raw_transcripts_included: false,
            final_text_included: false,
            credential_values_included: false,
            notes: vec![
                "Desktop logs are exported as a sanitized tail only.",
                "Only previously retained debug WAV recordings are included as audio samples; no new audio is captured during export.",
                "Firmware diag_log events are raw firmware diagnostic records without desktop transcript or credential values.",
            ],
        },
    };
    let manifest_bytes = serde_json::to_vec_pretty(&manifest)
        .map_err(|e| format!("生成诊断包 manifest 失败：{e}"))?;

    let file = File::create(target).map_err(|e| format!("创建诊断 zip 失败：{e}"))?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    write_zip_entry(&mut zip, "manifest.json", &manifest_bytes, options)?;
    write_zip_entry(
        &mut zip,
        "desktop/diagnostic_package.json",
        &desktop_package_bytes,
        options,
    )?;
    write_zip_entry(
        &mut zip,
        "desktop/listener-type-log-tail.txt",
        &log_tail_bytes,
        options,
    )?;
    write_zip_entry(
        &mut zip,
        "desktop/ble_connection_history.json",
        &ble_history_bytes,
        options,
    )?;
    write_zip_entry(
        &mut zip,
        "desktop/audio_samples_manifest.json",
        &audio_manifest_bytes,
        options,
    )?;
    write_zip_entry(
        &mut zip,
        "firmware/diag_log_summary.json",
        &firmware_summary_bytes,
        options,
    )?;
    if firmware_log.status == "ok" {
        write_zip_entry(
            &mut zip,
            "firmware/diag_log.bin",
            &firmware_log.raw_event_bytes,
            options,
        )?;
    }
    for sample in &audio_samples {
        write_zip_entry(&mut zip, &sample.zip_path, &sample.bytes, options)?;
    }
    zip.finish()
        .map(|_| ())
        .map_err(|e| format!("完成诊断 zip 失败：{e}"))
}

fn write_zip_entry<W: Write + std::io::Seek>(
    zip: &mut zip::ZipWriter<W>,
    path: &str,
    bytes: &[u8],
    options: zip::write::SimpleFileOptions,
) -> Result<(), String> {
    zip.start_file(path, options)
        .map_err(|e| format!("创建诊断 zip 条目 {path} 失败：{e}"))?;
    zip.write_all(bytes)
        .map_err(|e| format!("写入诊断 zip 条目 {path} 失败：{e}"))
}

fn sanitized_diagnostic_log_tail(lines: &[String]) -> String {
    lines
        .iter()
        .filter_map(|line| sanitize_diagnostic_log_line(line))
        .collect::<Vec<_>>()
        .join("\n")
}

fn diagnostic_ble_history(package: &DiagnosticPackage, lines: &[String]) -> Value {
    let ble_lines: Vec<String> = lines
        .iter()
        .filter(|line| {
            let lower = line.to_ascii_lowercase();
            lower.contains("ble") || lower.contains("bluetooth") || lower.contains("gatt")
        })
        .filter_map(|line| sanitize_diagnostic_log_line(line))
        .rev()
        .take(160)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    serde_json::json!({
        "capturedAt": &package.generated_at,
        "deviceAddress": &package.ble.device_address,
        "firmwareVersion": &package.ble.firmware_version,
        "batteryPercent": package.ble.battery_percent,
        "capabilities": &package.ble.capabilities,
        "backgroundListenerActive": package.ble.background_listener_active,
        "backgroundListenerReady": package.ble.background_listener_ready,
        "notifySubscriptionState": &package.ble.notify_subscription_state,
        "recentDisconnectReason": &package.ble.recent_disconnect_reason,
        "reconnectAttempts": package.ble.reconnect_attempts,
        "failureTaxonomy": &package.ble.failure_taxonomy,
        "diagnosticSnapshot": &package.ble.diagnostic_snapshot,
        "wakeRecovery": &package.ble.wake_recovery,
        "sessionActorHistory": &package.ble.session_actor_history,
        "logLines": ble_lines,
    })
}

fn diagnostic_audio_sample_files(package: &DiagnosticPackage) -> Vec<DiagnosticAudioSampleFile> {
    let mut samples = Vec::new();
    for session in package.recent_sessions.iter() {
        if samples.len() >= DIAGNOSTIC_AUDIO_SAMPLE_LIMIT {
            break;
        }
        if session.has_audio_recording != Some(true) || !is_valid_session_id(&session.id) {
            continue;
        }
        let Ok(path) = crate::persistence::recording_path_for_session(&session.id) else {
            continue;
        };
        let Ok(metadata) = std::fs::metadata(&path) else {
            continue;
        };
        if metadata.len() == 0 || metadata.len() > DIAGNOSTIC_AUDIO_SAMPLE_MAX_BYTES {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        samples.push(DiagnosticAudioSampleFile {
            session_id: session.id.clone(),
            zip_path: format!("desktop/audio_samples/{}.wav", session.id),
            bytes,
        });
    }
    samples
}

fn diagnostic_audio_samples_manifest(
    package: &DiagnosticPackage,
    audio_samples: &[DiagnosticAudioSampleFile],
) -> Value {
    let included_recordings: Vec<Value> = audio_samples
        .iter()
        .map(|sample| {
            serde_json::json!({
                "sessionId": &sample.session_id,
                "path": &sample.zip_path,
                "bytes": sample.bytes.len(),
            })
        })
        .collect();
    serde_json::json!({
        "capturedAt": &package.generated_at,
        "audioContentsIncluded": !audio_samples.is_empty(),
        "audioRecordingsIncluded": !audio_samples.is_empty(),
        "audioSampleLimit": DIAGNOSTIC_AUDIO_SAMPLE_LIMIT,
        "audioSampleMaxBytes": DIAGNOSTIC_AUDIO_SAMPLE_MAX_BYTES,
        "recordAudioForDebug": package.config.record_audio_for_debug,
        "audioRecordingMaxEntries": package.config.audio_recording_max_entries,
        "includedRecordings": included_recordings,
        "recentSessions": &package.recent_sessions,
        "latestEmbeddedSessionId": &package.ble.latest_session_id,
        "lastEmbeddedAudioEndReason": &package.ble.last_embedded_audio_end_reason,
        "lastEmbeddedAudioStats": &package.ble.last_embedded_audio_stats,
    })
}

fn build_diagnostic_package(coord: &Arc<Coordinator>) -> Result<DiagnosticPackage, String> {
    build_diagnostic_package_with_ble_snapshot(
        coord,
        crate::embedded_ble::ble_diagnostic_snapshot(),
    )
}

fn build_diagnostic_package_with_ble_snapshot(
    coord: &Arc<Coordinator>,
    ble_snapshot: crate::embedded_ble::BleDiagnosticSnapshot,
) -> Result<DiagnosticPackage, String> {
    let prefs = coord.prefs().get();
    let snap = CredentialsVault::snapshot();
    let active_asr_provider = CredentialsVault::get_active_asr();
    let active_llm_provider = CredentialsVault::get_active_llm();
    let asr_configured = asr_configured_for_provider(&active_asr_provider, &snap);
    let llm_configured = llm_configured_for_provider(&active_llm_provider, &snap);
    let history = coord.history().list().map_err(|e| e.to_string())?;
    let recent_sessions: Vec<DiagnosticRecentSession> = history
        .iter()
        .take(8)
        .map(diagnostic_recent_session)
        .collect();
    let embedded_sessions: Vec<&DiagnosticRecentSession> = recent_sessions
        .iter()
        .filter(|session| session.embedded_audio_stats.is_some())
        .collect();
    let latest_embedded = embedded_sessions.first().copied();
    let log_lines = read_diagnostic_log_tail(600);
    let timeline = diagnostic_timeline(&log_lines, 80);
    let recent_errors = diagnostic_recent_errors(&log_lines, 40);
    let wake_recovery = coord.embedded_ble_wake_recovery_snapshot();
    let failure_taxonomy = diagnostic_ble_failure_taxonomy(
        coord.embedded_ble_listener_last_error(),
        wake_recovery.recent_disconnect_reason.clone(),
        &recent_errors,
        &ble_snapshot,
    );

    Ok(DiagnosticPackage {
        schema_version: 3,
        generated_at: chrono::Utc::now().to_rfc3339(),
        app: DiagnosticApp {
            product_name: "Listener Type",
            identifier: "com.listener.type",
            version: env!("CARGO_PKG_VERSION"),
            log_path: crate::log_dir_path()
                .join("listener-type.log")
                .display()
                .to_string(),
            executable_path: std::env::current_exe()
                .ok()
                .map(|path| path.display().to_string()),
            windows_ime_status: crate::windows_ime_profile::get_windows_ime_status(),
            hotkey_status: coord.hotkey_status(),
        },
        platform: DiagnosticPlatform {
            os: std::env::consts::OS,
            family: std::env::consts::FAMILY,
            arch: std::env::consts::ARCH,
            debug_build: cfg!(debug_assertions),
        },
        firmware: DiagnosticFirmware {
            device_model: "VKA1",
            version: None,
            protocol: "VKA1 BLE audio",
            protocol_version: Some(1),
            readiness: Some(diagnostic_value(&wake_recovery.firmware_wake_policy)),
            wake_policy: wake_recovery.firmware_wake_policy.clone(),
            source: "Desktop readiness includes the accepted firmware 1.1 wake-policy contract; live DIS/readiness is populated by OTA preflight when available.",
        },
        ble: DiagnosticBle {
            input_source: diagnostic_value(&prefs.dictation_input_source),
            enabled_by_input_source: matches!(
                prefs.dictation_input_source,
                crate::types::DictationInputSource::EmbeddedBle
            ),
            background_listener_disabled_by_env: std::env::var("LISTENER_TYPE_DISABLE_BACKGROUND_BLE")
                .ok()
                .is_some_and(|value| value == "1"),
            background_listener_active: coord.embedded_ble_listener_active(),
            background_listener_ready: coord.embedded_ble_listener_ready(),
            background_listener_generation: coord.embedded_ble_listener_generation(),
            service_uuid: "710af845-6d9f-6583-0c4d-9e5b3bc3091a",
            device_address: ble_snapshot.configured_device_address.clone().or_else(|| {
                ble_snapshot
                    .audio_services
                    .iter()
                    .chain(ble_snapshot.ota_services.iter())
                    .find_map(|entry| entry.bluetooth_address.clone())
            }),
            firmware_version: ble_snapshot.firmware_snapshot.firmware_version.clone(),
            battery_percent: ble_snapshot.firmware_snapshot.battery_percent,
            capabilities: ble_snapshot.firmware_snapshot.capabilities.clone(),
            diagnostic_snapshot: ble_snapshot,
            failure_taxonomy,
            recent_disconnect_reason: wake_recovery.recent_disconnect_reason.clone(),
            reconnect_attempts: wake_recovery.reconnect_attempts,
            notify_subscription_state: format!("{:?}", wake_recovery.notify_subscription_state),
            wake_recovery,
            session_actor_history: coord.embedded_ble_session_actor_diagnostics(),
            recent_embedded_session_count: embedded_sessions.len(),
            latest_session_id: latest_embedded.map(|session| session.id.clone()),
            last_error_code: recent_sessions
                .iter()
                .find_map(|session| session.error_code.clone()),
            last_embedded_audio_end_reason: latest_embedded
                .and_then(|session| session.embedded_audio_stats.as_ref())
                .and_then(|stats| stats.end_reason.as_ref())
                .map(diagnostic_value),
            last_embedded_audio_stats: latest_embedded
                .and_then(|session| session.embedded_audio_stats.clone()),
        },
        config: DiagnosticConfig {
            dictation_input_source: diagnostic_value(&prefs.dictation_input_source),
            active_asr_provider: prefs.active_asr_provider.clone(),
            active_llm_provider: prefs.active_llm_provider.clone(),
            asr_configured,
            llm_configured,
            default_mode: diagnostic_value(&prefs.default_mode),
            active_style_pack_id: prefs.active_style_pack_id.clone(),
            enabled_modes: diagnostic_value(&prefs.enabled_modes),
            show_capsule: prefs.show_capsule,
            start_minimized: prefs.start_minimized,
            launch_at_login: prefs.launch_at_login,
            auto_update_check: prefs.auto_update_check,
            update_channel: diagnostic_value(&prefs.update_channel),
            history_retention_days: prefs.history_retention_days,
            history_max_entries: prefs.history_max_entries,
            record_audio_for_debug: prefs.record_audio_for_debug,
            audio_recording_max_entries: prefs.audio_recording_max_entries,
            local_asr_active_model: prefs.local_asr_active_model.clone(),
            local_asr_keep_loaded_secs: prefs.local_asr_keep_loaded_secs,
            foundry_local_asr_model: prefs.foundry_local_asr_model.clone(),
            foundry_local_runtime_source: prefs.foundry_local_runtime_source.clone(),
            foundry_local_asr_language_hint_configured: !prefs
                .foundry_local_asr_language_hint
                .trim()
                .is_empty(),
            foundry_local_asr_keep_loaded_secs: prefs.foundry_local_asr_keep_loaded_secs,
            microphone_device_configured: !prefs.microphone_device_name.trim().is_empty(),
            marketplace_backend_configured: !prefs.marketplace_base_url.trim().is_empty(),
        },
        credentials: DiagnosticCredentials {
            active_asr_provider: active_asr_provider.clone(),
            active_llm_provider: active_llm_provider.clone(),
            asr_configured,
            llm_configured,
            volcengine_configured: volcengine_configured(&snap),
            asr_api_key_configured: configured(&snap.asr_api_key),
            asr_endpoint_configured: configured(&snap.asr_endpoint),
            asr_model_configured: configured(&snap.asr_model),
            llm_api_key_configured: configured(&snap.ark_api_key),
            llm_endpoint_configured: configured(&snap.ark_endpoint),
            llm_model_configured: configured(&snap.ark_model_id),
            codex_oauth_configured: CodexOAuthCredentials::load_default().is_ok(),
        },
        recent_errors,
        timeline,
        recent_sessions,
        privacy: DiagnosticPrivacy {
            excludes_audio_contents: true,
            excludes_audio_recordings: true,
            excludes_raw_transcripts: true,
            excludes_final_text: true,
            excludes_api_key_values: true,
            credential_values_exported: false,
            notes: vec![
                "History rawTranscript/finalText fields are not exported.",
                "This JSON excludes WAV/audio bytes; the surrounding ZIP may include previously retained debug WAV samples when available.",
                "Credential values, API keys, access tokens and OAuth tokens are not exported.",
            ],
        },
    })
}

fn diagnostic_recent_session(session: &DictationSession) -> DiagnosticRecentSession {
    DiagnosticRecentSession {
        id: session.id.clone(),
        created_at: session.created_at.clone(),
        mode: diagnostic_value(&session.mode),
        insert_status: diagnostic_value(&session.insert_status),
        error_code: session.error_code.clone(),
        duration_ms: session.duration_ms,
        dictionary_entry_count: session.dictionary_entry_count,
        has_audio_recording: session.has_audio_recording,
        embedded_audio_stats: session.embedded_audio_stats.clone(),
    }
}

fn diagnostic_value<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

fn diagnostic_ble_failure_taxonomy(
    listener_last_error: Option<String>,
    recent_disconnect_reason: Option<String>,
    recent_errors: &[String],
    snapshot: &crate::embedded_ble::BleDiagnosticSnapshot,
) -> Vec<crate::embedded_ble::BleFailureClassification> {
    let mut messages = Vec::new();
    if let Some(value) = listener_last_error {
        messages.push(value);
    }
    if let Some(value) = recent_disconnect_reason {
        messages.push(value);
    }
    messages.extend(recent_errors.iter().cloned());
    messages.extend(snapshot.errors.iter().cloned());
    if snapshot.firmware_snapshot.connected && snapshot.firmware_snapshot.firmware_version.is_none()
    {
        messages.push("DIS firmware revision missing from live firmware snapshot".to_string());
    }
    if snapshot.audio_services.is_empty() && snapshot.ota_services.is_empty() {
        messages.push("Listener BLE service selectors returned no devices".to_string());
    }

    let mut classifications = Vec::new();
    for message in messages {
        let classification = crate::embedded_ble::classify_ble_failure(&message);
        if !classifications.iter().any(
            |existing: &crate::embedded_ble::BleFailureClassification| {
                existing.kind == classification.kind && existing.evidence == classification.evidence
            },
        ) {
            classifications.push(classification);
        }
    }

    classifications
}

fn read_diagnostic_log_tail(max_lines: usize) -> Vec<String> {
    let path = crate::log_dir_path().join("listener-type.log");
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut lines: Vec<String> = content
        .lines()
        .rev()
        .take(max_lines)
        .map(|line| line.to_string())
        .collect();
    lines.reverse();
    lines
}

fn diagnostic_timeline(lines: &[String], max_lines: usize) -> Vec<String> {
    let mut selected: Vec<String> = lines
        .iter()
        .filter(|line| is_diagnostic_timeline_line(line))
        .filter_map(|line| sanitize_diagnostic_log_line(line))
        .collect();
    if selected.len() > max_lines {
        selected = selected.split_off(selected.len() - max_lines);
    }
    selected
}

fn diagnostic_recent_errors(lines: &[String], max_lines: usize) -> Vec<String> {
    let mut selected: Vec<String> = lines
        .iter()
        .filter(|line| is_diagnostic_error_line(line))
        .filter_map(|line| sanitize_diagnostic_log_line(line))
        .collect();
    if selected.len() > max_lines {
        selected = selected.split_off(selected.len() - max_lines);
    }
    selected
}

fn is_diagnostic_timeline_line(line: &str) -> bool {
    const MARKERS: &[&str] = &[
        "[startup]",
        "[embedded-ble]",
        "[coord]",
        "[asr]",
        "[foundry-asr]",
        "[local-asr]",
        "[windows-ime]",
        "[capsule]",
        "[qa]",
        "ERROR",
        "WARN",
    ];
    MARKERS.iter().any(|marker| line.contains(marker))
}

fn is_diagnostic_error_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.contains("error")
        || lower.contains("warn")
        || lower.contains("failed")
        || lower.contains("timeout")
        || lower.contains("panic")
}

fn sanitize_diagnostic_log_line(line: &str) -> Option<String> {
    if diagnostic_line_may_contain_user_text(line) {
        return None;
    }
    let redacted = redact_diagnostic_log_line(line);
    let trimmed: String = redacted.chars().take(480).collect();
    Some(trimmed)
}

fn diagnostic_line_may_contain_user_text(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    const TEXT_MARKERS: &[&str] = &[
        "rawtranscript",
        "raw_transcript",
        "raw transcript",
        "finaltext",
        "final_text",
        "final text",
        "transcript=",
        "transcript:",
    ];
    TEXT_MARKERS.iter().any(|marker| lower.contains(marker))
}

fn redact_diagnostic_log_line(line: &str) -> String {
    let mut redact_next = false;
    line.split_whitespace()
        .map(|token| {
            if redact_next {
                redact_next = false;
                return "[redacted]".to_string();
            }
            let lower = token.to_ascii_lowercase();
            if lower == "bearer" || lower == "authorization:" || lower == "authorization" {
                redact_next = true;
                return "[redacted]".to_string();
            }
            if diagnostic_token_contains_secret(&lower) {
                if !(token.contains('=') || token.contains(':')) {
                    redact_next = true;
                }
                return "[redacted]".to_string();
            }
            if looks_like_secret_token(token) {
                return "[redacted]".to_string();
            }
            token.to_string()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn diagnostic_token_contains_secret(lower: &str) -> bool {
    const SECRET_MARKERS: &[&str] = &[
        "api_key",
        "apikey",
        "x-goog-api-key",
        "access_key",
        "accesskey",
        "secret_key",
        "secretkey",
        "access_token",
        "accesstoken",
        "refresh_token",
        "refreshtoken",
    ];
    SECRET_MARKERS.iter().any(|marker| lower.contains(marker))
}

fn looks_like_secret_token(token: &str) -> bool {
    token.starts_with("sk-")
        || token.starts_with("eyJ")
        || token.starts_with("ya29.")
        || token.len() > 72
            && token
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || "-_.".contains(ch))
}

// ─────────────────────────── unused but exported (silences dead_code) ───────────────────────────

#[allow(dead_code)]
fn _ensure_snapshot_used(_: CredentialsSnapshot) {}
