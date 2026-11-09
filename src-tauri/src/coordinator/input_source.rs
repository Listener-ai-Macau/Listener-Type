//! Automatic routing changes the next capture, never the source of an active one.
use super::*;

#[derive(Default)]
pub(super) struct Runtime {
    started: AtomicBool,
    notify: Notify,
}

pub(super) fn wants_device_listener(prefs: &UserPreferences) -> bool {
    !prefs.dictation_input_source_user_overridden
        || prefs.dictation_input_source == DictationInputSource::EmbeddedBle
}

fn desired_source(prefs: &UserPreferences, device_ready: bool) -> DictationInputSource {
    if prefs.dictation_input_source_user_overridden {
        prefs.dictation_input_source
    } else if device_ready {
        DictationInputSource::EmbeddedBle
    } else {
        DictationInputSource::Microphone
    }
}

impl Runtime {
    pub(super) fn refresh(&self) {
        self.notify.notify_one();
    }

    pub(super) fn start(self: &Arc<Self>, inner: std::sync::Weak<Inner>) {
        if self.started.swap(true, Ordering::SeqCst) { return; }
        let runtime = Arc::clone(self);
        async_runtime::spawn(async move {
            loop {
                let notified = runtime.notify.notified();
                let Some(current) = inner.upgrade() else { break; };
                if current.shutdown.load(Ordering::SeqCst) { break; }
                let deferred = reconcile(&current);
                drop(current);
                if deferred {
                    // Poll only while a real route change awaits capture drain.
                    tokio::select! {
                        _ = notified => {},
                        _ = tokio::time::sleep(Duration::from_millis(400)) => {},
                    }
                } else {
                    notified.await;
                }
            }
        });
    }
}

/// Returns true only when a changed source must wait for existing work to finish.
pub(super) fn reconcile(inner: &Arc<Inner>) -> bool {
    let capture = inner.state.lock();
    let question = inner.qa_state.lock();
    let prefs = inner.prefs.get();
    let desired = desired_source(&prefs, inner.embedded_ble_listener_ready.load(Ordering::SeqCst));
    if desired == prefs.dictation_input_source { return false; }
    if capture.phase != SessionPhase::Idle || question.phase != qa::QaPhase::Idle
        || inner.embedded_ble_ota_active.load(Ordering::SeqCst)
        || inner.shutdown.load(Ordering::SeqCst)
    { return true; }
    // The store rechecks manual override under its write lock. A concurrent
    // settings save cannot be replaced by an older whole-preferences snapshot.
    let changed = inner.prefs.set_automatic_input_source(desired);
    drop(question);
    drop(capture);
    let prefs = match changed {
        Ok(Some(prefs)) => prefs,
        Ok(None) => return false,
        Err(error) => {
            log::warn!("[input-source] automatic selection not saved: {error}");
            return false;
        }
    };
    log::info!("[input-source] automatic source={:?} device_ready={}",
        prefs.dictation_input_source, inner.embedded_ble_listener_ready.load(Ordering::SeqCst));
    #[cfg(target_os = "windows")]
    {
        if prefs.microphone_wake_enabled {
            inner.microphone_wake.start_supervisor(Arc::downgrade(inner));
        }
        inner.microphone_wake.refresh();
    }
    if let Some(app) = inner.app.lock().clone() {
        let _ = app.emit("prefs:changed", &prefs);
        let app_for_main = app.clone();
        let _ = app.run_on_main_thread(move || {
            if let Err(error) = crate::refresh_tray_microphone_menu(&app_for_main) {
                log::warn!("[input-source] tray refresh failed: {error}");
            }
        });
    }
    // The device listener stays attached in automatic mode, including while
    // computer input is selected. Switching never restarts that same link.
    false
}

#[cfg(test)]
#[path = "input_source_tests.rs"]
mod tests;
