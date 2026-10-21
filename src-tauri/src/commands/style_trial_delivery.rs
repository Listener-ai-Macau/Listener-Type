// Explicit text-workspace handoff; independent of recording and its history.
#[derive(Default)]
struct StyleInsertRegistry {
    pending: Option<(uuid::Uuid, Arc<crate::insertion::ExplicitPastePermit>)>,
    cancelled: std::collections::VecDeque<uuid::Uuid>,
}
impl StyleInsertRegistry {
    fn arm(&mut self, id: uuid::Uuid, permit: Arc<crate::insertion::ExplicitPastePermit>) -> Result<(), String> {
        if self.cancelled.contains(&id) { return Err("style_insert_cancelled".into()); }
        if self.pending.is_some() { return Err("style_insert_busy".into()); }
        self.pending = Some((id, permit));
        Ok(())
    }
    fn cancel(&mut self, id: uuid::Uuid) -> bool {
        if let Some((_,permit)) = self.pending.as_ref().filter(|(current,_)| *current == id) {
            if !permit.cancel() { return false; }
            self.pending = None;
        }
        // IPC cancellation can arrive before the async start is first polled.
        // Remember those IDs without touching a different pending request.
        if !self.cancelled.contains(&id) { self.cancelled.push_back(id); }
        while self.cancelled.len() > 64 { self.cancelled.pop_front(); }
        true
    }
    fn finish(&mut self, id: uuid::Uuid) {
        if self.pending.as_ref().is_some_and(|(current,_)| *current == id) { self.pending = None; }
    }
}
static STYLE_INSERT: OnceLock<Mutex<StyleInsertRegistry>> = OnceLock::new();

#[tauri::command]
pub async fn insert_style_trial_result(window: Window, coord: CoordinatorState<'_>,
    request_id: String, text: String) -> Result<String, String> {
    ensure_main_window(&window)?;
    if !cfg!(target_os = "windows") { return Err("style_insert_unsupported".into()); }
    if text.is_empty() || text.chars().count() > 100_000 { return Err("style_insert_invalid".into()); }
    let id = uuid::Uuid::parse_str(&request_id).map_err(|_| "style_insert_invalid")?;
    if coord.dictation_phase_for_cli() != crate::coordinator_state::SessionPhase::Idle {
        return Err("style_insert_recording".into());
    }
    let permit = Arc::new(crate::insertion::ExplicitPastePermit::new(Duration::from_secs(8)));
    let pending = STYLE_INSERT.get_or_init(|| Mutex::new(StyleInsertRegistry::default()));
    pending.lock().arm(id, Arc::clone(&permit))?;
    let origin = crate::insertion::TextInserter::new().clipboard_retention_ticket();
    tokio::time::sleep(Duration::from_secs(5)).await;
    let coord = Arc::clone(&coord);
    let result = tauri::async_runtime::spawn_blocking(move || {
        if coord.dictation_phase_for_cli() != crate::coordinator_state::SessionPhase::Idle {
            permit.cancel();
            return Err("style_insert_recording".to_owned());
        }
        let target = crate::insertion::explicit_external_target().ok_or("style_insert_no_target")?;
        let mut submission = crate::insertion::paste_explicit_target(&text,
            coord.prefs().get().paste_shortcut, origin, target, permit);
        if !matches!(submission.status, crate::types::InsertStatus::PasteSent | crate::types::InsertStatus::Inserted) {
            return Err("style_insert_no_target".into());
        }
        let until = Instant::now() + Duration::from_millis(1500);
        while Instant::now() < until {
            let Some(receipt) = submission.receipt.as_mut() else { break; };
            match receipt.try_take() {
                crate::insertion::PasteReceiptPoll::Ready(done) => {
                    return Ok(if matches!(done.outcome, crate::insertion::PasteReceiptOutcome::Confirmed(_)) {
                        "confirmed" } else { "sent" }.to_owned());
                }
                crate::insertion::PasteReceiptPoll::Pending => std::thread::sleep(Duration::from_millis(25)),
                crate::insertion::PasteReceiptPoll::Consumed => break,
            }
        }
        Ok("sent".to_owned())
    }).await.map_err(|_| "style_insert_failed".to_owned()).and_then(|value| value);
    pending.lock().finish(id);
    result
}

#[tauri::command]
pub fn cancel_style_trial_insert(window: Window, request_id: String) -> Result<bool, String> {
    ensure_main_window(&window)?;
    let id = uuid::Uuid::parse_str(&request_id).map_err(|_| "style_insert_invalid")?;
    Ok(STYLE_INSERT.get_or_init(|| Mutex::new(StyleInsertRegistry::default())).lock().cancel(id))
}

#[cfg(test)]
mod style_insert_registry_tests {
    use super::*;
    #[test]
    fn explicit_handoff_close_before_start_and_old_finish_cannot_insert_or_erase_new_work() {
        let mut registry = StyleInsertRegistry::default();
        let old = uuid::Uuid::new_v4(); let next = uuid::Uuid::new_v4();
        assert!(registry.cancel(old));
        assert!(registry.arm(old, Arc::new(crate::insertion::ExplicitPastePermit::new(Duration::from_secs(8)))).is_err());
        registry.arm(next, Arc::new(crate::insertion::ExplicitPastePermit::new(Duration::from_secs(8)))).unwrap();
        registry.finish(old);
        registry.cancel(old);
        assert_eq!(registry.pending.as_ref().map(|(id,_)| *id), Some(next));
        assert!(registry.cancel(next));
        assert!(registry.pending.is_none());
    }
}
