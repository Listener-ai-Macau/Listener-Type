use std::fs::{self, OpenOptions};
use std::io::Write;
use std::sync::Mutex;

use chrono::Utc;
use serde_json::{json, Value};

use crate::types::CapsulePayload;

const CAPSULE_LOG_FILE: &str = "capsule-timeline.log";
const CAPSULE_LOG_ARCHIVE_FILE: &str = "capsule-timeline.log.1";
const CAPSULE_LOG_ROTATE_LIMIT_BYTES: u64 = 2 * 1024 * 1024;

static CAPSULE_LOG_LOCK: Mutex<()> = Mutex::new(());

pub(crate) fn record_backend_emit(payload: &CapsulePayload, visible: bool, show_capsule: bool) {
    append_record(backend_emit_record(payload, visible, show_capsule));
}

fn backend_emit_record(payload: &CapsulePayload, visible: bool, show_capsule: bool) -> Value {
    let state = serde_json::to_value(payload.state)
        .ok()
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .unwrap_or_else(|| format!("{:?}", payload.state));
    json!({
        "ts": Utc::now().to_rfc3339(),
        "source": "backend.capsule",
        "event": "emit",
        "seq": payload.seq,
        "sessionId": payload.session_id.as_deref(),
        "state": state,
        "elapsedMs": payload.elapsed_ms,
        "visible": visible,
        "showCapsule": show_capsule,
        "hasMessage": payload.message.is_some(),
        "messageChars": payload.message.as_deref().map(|value| value.chars().count()),
        "insertedChars": payload.inserted_chars,
        "translation": payload.translation,
    })
}

pub(crate) fn record_ui_event(
    source: &str,
    event: &str,
    state: Option<&str>,
    elapsed_ms: Option<u64>,
    detail: Option<&Value>,
) {
    append_record(json!({
        "ts": Utc::now().to_rfc3339(),
        "source": source,
        "event": event,
        "state": state,
        "elapsedMs": elapsed_ms,
        "detail": detail.map(sanitize_diagnostic_value).unwrap_or_else(|| json!({})),
    }));
}

pub(crate) fn sanitize_diagnostic_value(value: &Value) -> Value {
    match value {
        Value::Object(values) => Value::Object(
            values
                .iter()
                .map(|(key, value)| {
                    let normalized = key.to_ascii_lowercase();
                    let sensitive = [
                        "message",
                        "text",
                        "transcript",
                        "payload",
                        "json",
                        "token",
                        "secret",
                        "authorization",
                        "apikey",
                        "accesskey",
                        "appkey",
                        "credential",
                    ]
                    .iter()
                    .any(|name| normalized == *name || normalized.ends_with(name));
                    (
                        key.clone(),
                        if sensitive {
                            Value::String("<redacted>".into())
                        } else {
                            sanitize_diagnostic_value(value)
                        },
                    )
                })
                .collect(),
        ),
        Value::Array(values) => {
            Value::Array(values.iter().map(sanitize_diagnostic_value).collect())
        }
        _ => value.clone(),
    }
}

fn append_record(record: Value) {
    let Ok(_guard) = CAPSULE_LOG_LOCK.lock() else {
        return;
    };
    let log_dir = crate::log_dir_path();
    if let Err(err) = fs::create_dir_all(&log_dir) {
        log::warn!("[capsule-log] create log dir failed: {err}");
        return;
    }
    let path = log_dir.join(CAPSULE_LOG_FILE);
    if let Err(err) = rotate_if_needed(&path) {
        log::warn!("[capsule-log] rotate failed: {err}");
    }
    let mut file = match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(file) => file,
        Err(err) => {
            log::warn!("[capsule-log] open failed: {err}");
            return;
        }
    };
    if let Err(err) = writeln!(file, "{record}") {
        log::warn!("[capsule-log] write failed: {err}");
    }
}

fn rotate_if_needed(path: &std::path::Path) -> std::io::Result<()> {
    let Ok(metadata) = fs::metadata(path) else {
        return Ok(());
    };
    if metadata.len() <= CAPSULE_LOG_ROTATE_LIMIT_BYTES {
        return Ok(());
    }

    let archive = path.with_file_name(CAPSULE_LOG_ARCHIVE_FILE);
    match fs::remove_file(&archive) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }
    fs::rename(path, archive)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::CapsuleState;

    #[test]
    fn backend_capsule_record_contains_lengths_but_not_transcript_plaintext() {
        let payload = CapsulePayload {
            seq: 7,
            session_id: Some("session-1".into()),
            state: CapsuleState::Recording,
            level: 0.5,
            elapsed_ms: 900,
            message: Some("这是一段不能进入诊断日志的正文".into()),
            inserted_chars: None,
            translation: false,
        };
        let record = backend_emit_record(&payload, true, true).to_string();
        assert!(!record.contains("这是一段不能进入诊断日志的正文"));
        assert!(record.contains("messageChars"));
    }

    #[test]
    fn diagnostic_detail_redacts_nested_transcripts_and_secrets() {
        let sanitized = sanitize_diagnostic_value(&json!({
            "sessionId": "safe-id",
            "nested": {
                "transcript": "private speech",
                "accessToken": "secret-token",
                "reason": "timeout"
            }
        }));
        let serialized = sanitized.to_string();
        assert!(serialized.contains("safe-id"));
        assert!(serialized.contains("timeout"));
        assert!(!serialized.contains("private speech"));
        assert!(!serialized.contains("secret-token"));
    }
}
