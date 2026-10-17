// Observation only. Keep the accepted capture transaction with its own audio
// archive: the rotating application log may expire before a failure is read.
// None of these fields are consulted by recording/identity/endpoint policy.
#[derive(Clone)]
struct CaptureControlTrace {
    inner: Arc<std::sync::Mutex<CaptureControlTraceState>>,
}

struct CaptureControlTraceState {
    started_at: Instant,
    request_id: u32,
    previous_segment_id: u32,
    write_started_ms: Option<u64>,
    write_completed_ms: Option<u64>,
    write_error: Option<String>,
    confirmed_segment_id: Option<u32>,
    confirmed_ms: Option<u64>,
    retired_reason: Option<&'static str>,
    retired_ms: Option<u64>,
}

impl CaptureControlTrace {
    fn new(request_id: u32, previous_segment_id: u32) -> Self {
        Self { inner: Arc::new(std::sync::Mutex::new(CaptureControlTraceState {
            started_at: Instant::now(), request_id, previous_segment_id,
            write_started_ms: None, write_completed_ms: None, write_error: None,
            confirmed_segment_id: None, confirmed_ms: None,
            retired_reason: None, retired_ms: None,
        })) }
    }

    fn write_started(&self) {
        if let Ok(mut state) = self.inner.lock() {
            state.write_started_ms = Some(state.started_at.elapsed().as_millis() as u64);
        }
    }

    fn write_completed(&self, result: &Result<(), String>) {
        if let Ok(mut state) = self.inner.lock() {
            state.write_completed_ms = Some(state.started_at.elapsed().as_millis() as u64);
            state.write_error = result.as_ref().err().cloned();
        }
    }

    fn confirmed(&self, request_id: u32, segment_id: u32) {
        if let Ok(mut state) = self.inner.lock() {
            if state.request_id != request_id { return; }
            state.confirmed_segment_id = Some(segment_id);
            state.confirmed_ms = Some(state.started_at.elapsed().as_millis() as u64);
        }
    }

    fn retired(&self, request_id: u32, reason: &'static str) {
        if let Ok(mut state) = self.inner.lock() {
            if state.request_id != request_id { return; }
            state.retired_reason = Some(reason);
            state.retired_ms = Some(state.started_at.elapsed().as_millis() as u64);
        }
    }

    fn snapshot(&self) -> serde_json::Value {
        let Ok(state) = self.inner.lock() else {
            return serde_json::json!({"observationUnavailable": true});
        };
        serde_json::json!({
            "requestId": state.request_id,
            "previousSegmentId": state.previous_segment_id,
            "writeStartedMs": state.write_started_ms,
            "writeCompletedMs": state.write_completed_ms,
            "writeResult": if state.write_completed_ms.is_none() { "pending" }
                else if state.write_error.is_some() { "error" } else { "sent" },
            "writeError": state.write_error,
            "confirmedSegmentId": state.confirmed_segment_id,
            "confirmedMs": state.confirmed_ms,
            "retiredReason": state.retired_reason,
            "retiredMs": state.retired_ms,
        })
    }
}

#[cfg(test)]
mod capture_control_trace_tests {
    use super::*;

    #[test]
    fn retained_audio_archive_observes_detached_write_failure() {
        let owner = CaptureControlTrace::new(41, 7548);
        let writer = owner.clone();
        writer.write_started();
        writer.write_completed(&Err("active Listener BLE audio control request expired in queue: accepted wake capture ensure".into()));
        drop(writer);
        owner.retired(41, "ended_source_without_continuation");
        let archive = serde_json::json!({"captureControl": owner.snapshot()});
        let saved = serde_json::to_vec(&archive).unwrap();
        let reread: serde_json::Value = serde_json::from_slice(&saved).unwrap();
        assert_eq!(reread["captureControl"]["writeResult"], "error");
        assert!(reread["captureControl"]["writeError"].as_str().unwrap().contains("expired in queue"));
        assert_eq!(reread["captureControl"]["retiredReason"], "ended_source_without_continuation");
        assert!(reread["captureControl"]["confirmedSegmentId"].is_null());
    }

    #[test]
    fn sent_command_does_not_invent_device_confirmation() {
        let trace = CaptureControlTrace::new(42, 100);
        assert_eq!(trace.snapshot()["writeResult"], "pending");
        trace.write_started(); trace.write_completed(&Ok(()));
        assert_eq!(trace.snapshot()["writeResult"], "sent");
        assert!(trace.snapshot()["confirmedSegmentId"].is_null());
        trace.confirmed(42, 101);
        assert_eq!(trace.snapshot()["confirmedSegmentId"], 101);
    }

    #[test]
    fn old_request_cannot_confirm_or_retire_a_new_recording() {
        let trace = CaptureControlTrace::new(43, 200);
        trace.confirmed(42, 201); trace.retired(42, "user_cancel");
        assert!(trace.snapshot()["confirmedMs"].is_null());
        assert!(trace.snapshot()["retiredReason"].is_null());
    }

    #[test]
    fn late_writer_is_owned_by_original_audio_session() {
        let old = CaptureControlTrace::new(44, 300);
        let writer = old.clone();
        let new = CaptureControlTrace::new(45, 301);
        old.retired(44, "user_cancel");
        writer.write_started(); writer.write_completed(&Ok(()));
        assert_eq!(old.snapshot()["writeResult"], "sent");
        assert_eq!(old.snapshot()["retiredReason"], "user_cancel");
        assert_eq!(new.snapshot()["requestId"], 45);
        assert_eq!(new.snapshot()["writeResult"], "pending");
    }
}
