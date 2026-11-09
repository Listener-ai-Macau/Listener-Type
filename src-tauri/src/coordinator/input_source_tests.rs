use super::*;

#[test]
fn automatic_source_does_not_wait_on_qa_while_holding_dictation() {
    let coordinator = Coordinator::new();
    let inner = &coordinator.inner;
    let mut prefs = UserPreferences::default();
    prefs.dictation_input_source = DictationInputSource::Microphone;
    inner.prefs.replace_for_tests(prefs);
    inner.embedded_ble_listener_ready.store(true, Ordering::SeqCst);
    let qa = inner.qa_state.lock();
    let (tx, rx) = std::sync::mpsc::channel();
    let shared = Arc::clone(inner);
    let worker = std::thread::spawn(move || { let _ = tx.send(reconcile(&shared)); });
    let result = rx.recv_timeout(Duration::from_secs(1));
    // Recreate the existing QA-idle order before releasing its QA lock.
    let dictation_available = inner.state.try_lock().is_some();
    drop(qa);
    worker.join().unwrap();
    assert_eq!(result.ok(), Some(true));
    assert!(dictation_available);
    assert_eq!(inner.prefs.get().dictation_input_source, DictationInputSource::Microphone);
}

#[test]
fn enabling_automatic_mode_keeps_an_existing_device_capture_attached() {
    let coordinator = Coordinator::new();
    let inner = &coordinator.inner;
    let prefs = UserPreferences::default();
    inner.prefs.replace_for_tests(prefs);
    inner.state.lock().phase = SessionPhase::Listening;
    let cancel = Arc::new(AtomicBool::new(false));
    *inner.embedded_ble_listener_cancel.lock() = Some(Arc::clone(&cancel));
    let generation = inner.embedded_ble_listener_generation.load(Ordering::SeqCst);
    coordinator.refresh_input_source_device_listener();
    assert!(!cancel.load(Ordering::SeqCst));
    assert_eq!(inner.embedded_ble_listener_generation.load(Ordering::SeqCst), generation);
    assert!(inner.embedded_ble_listener_cancel.lock().as_ref().is_some_and(|current| Arc::ptr_eq(current, &cancel)));
}

#[tokio::test]
async fn automatic_source_waits_for_drain_and_receives_the_disconnect_edge() {
    let coordinator = Coordinator::new();
    let inner = &coordinator.inner;
    let mut prefs = UserPreferences::default();
    prefs.microphone_wake_enabled = false;
    prefs.dictation_input_source = DictationInputSource::Microphone;
    inner.prefs.replace_for_tests(prefs);
    inner.state.lock().phase = SessionPhase::Listening;
    inner.embedded_ble_listener_ready.store(true, Ordering::SeqCst);
    inner.input_source.start(Arc::downgrade(inner));
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(inner.prefs.get().dictation_input_source, DictationInputSource::Microphone);
    inner.state.lock().phase = SessionPhase::Idle;
    let deadline = Instant::now() + Duration::from_secs(3);
    while inner.prefs.get().dictation_input_source != DictationInputSource::EmbeddedBle {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    inner.embedded_ble_listener_ready.store(false, Ordering::SeqCst);
    inner.input_source.refresh();
    let deadline = Instant::now() + Duration::from_secs(3);
    while inner.prefs.get().dictation_input_source != DictationInputSource::Microphone {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    inner.shutdown.store(true, Ordering::SeqCst);
    inner.input_source.refresh();
}

#[test]
fn automatic_route_tracks_ready_link_and_keeps_discovery_in_computer_mode() {
    let mut prefs = UserPreferences::default();
    prefs.dictation_input_source = DictationInputSource::Microphone;
    assert_eq!(desired_source(&prefs, true), DictationInputSource::EmbeddedBle);
    assert!(wants_device_listener(&prefs));
    prefs.dictation_input_source = DictationInputSource::EmbeddedBle;
    assert_eq!(desired_source(&prefs, false), DictationInputSource::Microphone);
    for source in [DictationInputSource::Microphone, DictationInputSource::EmbeddedBle] {
        prefs.dictation_input_source = source;
        prefs.dictation_input_source_user_overridden = true;
        for ready in [false, true] { assert_eq!(desired_source(&prefs, ready), source); }
        assert_eq!(wants_device_listener(&prefs), source == DictationInputSource::EmbeddedBle);
    }
}

#[test]
fn automatic_source_defers_active_work_then_routes_both_directions_without_reopening_link() {
    let coordinator = Coordinator::new();
    let inner = &coordinator.inner;
    let mut prefs = UserPreferences::default();
    prefs.microphone_wake_enabled = false;
    prefs.dictation_input_source = DictationInputSource::Microphone;
    let final_copy = prefs.copy_dictation_to_clipboard;
    inner.prefs.replace_for_tests(prefs.clone());
    let generation = inner.embedded_ble_listener_generation.load(Ordering::SeqCst);
    inner.embedded_ble_listener_ready.store(true, Ordering::SeqCst);
    for phase in [SessionPhase::Starting, SessionPhase::Listening, SessionPhase::Processing, SessionPhase::Inserting] {
        inner.state.lock().phase = phase;
        assert!(reconcile(inner));
        assert_eq!(inner.prefs.get().dictation_input_source, DictationInputSource::Microphone);
    }
    inner.state.lock().phase = SessionPhase::Idle;
    for phase in [qa::QaPhase::Recording, qa::QaPhase::Processing] {
        inner.qa_state.lock().phase = phase;
        assert!(reconcile(inner));
    }
    inner.qa_state.lock().phase = qa::QaPhase::Idle;
    inner.embedded_ble_ota_active.store(true, Ordering::SeqCst);
    assert!(reconcile(inner));
    inner.embedded_ble_ota_active.store(false, Ordering::SeqCst);
    assert!(!reconcile(inner));
    assert_eq!(inner.prefs.get().dictation_input_source, DictationInputSource::EmbeddedBle);
    // An unrelated save from an older UI snapshot cannot revert the route.
    prefs.history_retention_days = 42;
    inner.prefs.set(prefs).unwrap();
    assert_eq!(inner.prefs.get().dictation_input_source, DictationInputSource::EmbeddedBle);
    assert_eq!(inner.prefs.get().history_retention_days, 42);
    inner.embedded_ble_listener_ready.store(false, Ordering::SeqCst);
    inner.state.lock().phase = SessionPhase::Processing;
    assert!(reconcile(inner));
    assert_eq!(inner.prefs.get().dictation_input_source, DictationInputSource::EmbeddedBle);
    inner.state.lock().phase = SessionPhase::Idle;
    assert!(!reconcile(inner));
    let prefs = inner.prefs.get();
    assert_eq!(prefs.dictation_input_source, DictationInputSource::Microphone);
    assert!(!prefs.dictation_input_source_user_overridden);
    assert_eq!(prefs.copy_dictation_to_clipboard, final_copy);
    assert_eq!(inner.embedded_ble_listener_generation.load(Ordering::SeqCst), generation);
    let mut manual = prefs;
    manual.dictation_input_source_user_overridden = true;
    inner.prefs.set(manual).unwrap();
    inner.embedded_ble_listener_ready.store(true, Ordering::SeqCst);
    assert!(!reconcile(inner));
    assert!(inner.prefs.set_automatic_input_source(DictationInputSource::EmbeddedBle).unwrap().is_none());
    assert_eq!(inner.prefs.get().dictation_input_source, DictationInputSource::Microphone);
}
