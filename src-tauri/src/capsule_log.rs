use std::fs::{self, OpenOptions};
use std::io::Write;
use std::sync::Mutex;

use chrono::Utc;
use serde_json::{json, Value};

use crate::types::CapsulePayload;

const CAPSULE_LOG_FILE: &str = "capsule-timeline.log";
const CAPSULE_LOG_ARCHIVE_FILE: &str = "capsule-timeline.log.1";
const CAPSULE_LOG_ROTATE_LIMIT_BYTES: u64 = 2 * 1024 * 1024;
const MAX_MESSAGE_CHARS: usize = 180;

static CAPSULE_LOG_LOCK: Mutex<()> = Mutex::new(());

pub(crate) fn record_backend_emit(payload: &CapsulePayload, visible: bool, show_capsule: bool) {
    let state = serde_json::to_value(payload.state)
        .ok()
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .unwrap_or_else(|| format!("{:?}", payload.state));
    append_record(json!({
        "ts": Utc::now().to_rfc3339(),
        "source": "backend.capsule",
        "event": "emit",
        "seq": payload.seq,
        "sessionId": payload.session_id.as_deref(),
        "state": state,
        "elapsedMs": payload.elapsed_ms,
        "visible": visible,
        "showCapsule": show_capsule,
        "message": payload.message.as_deref().map(truncate_message),
        "insertedChars": payload.inserted_chars,
        "translation": payload.translation,
    }));
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
        "detail": detail.cloned().unwrap_or_else(|| json!({})),
    }));
}

fn truncate_message(value: &str) -> String {
    let mut chars = value.chars();
    let truncated: String = chars.by_ref().take(MAX_MESSAGE_CHARS).collect();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
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
