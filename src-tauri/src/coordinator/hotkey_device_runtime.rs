// Hotkey supervisors and device-key free functions.
// Included into `coordinator` via `include!` for soft-budget maintainability.

fn hotkey_supervisor_loop(inner: Arc<Inner>) {
    let mut attempts: u32 = 0;
    let capability = HotkeyMonitor::capability();
    loop {
        if inner.shutdown.load(Ordering::SeqCst) {
            return;
        }
        let prefs = inner.prefs.get();

        if inner.hotkey.lock().is_some() {
            return;
        }
        *inner.hotkey_status.lock() = HotkeyStatus {
            adapter: capability.adapter,
            state: HotkeyStatusState::Starting,
            message: Some(format!("正在安装全局快捷键监听（第 {} 次）", attempts + 1)),
            last_error: None,
        };
        let (tx, rx) = mpsc::channel::<HotkeyEvent>();
        let trigger = crate::shortcut_binding::legacy_modifier_trigger(&prefs.dictation_hotkey)
            .unwrap_or(crate::types::HotkeyTrigger::Custom);
        let binding = crate::types::HotkeyBinding {
            trigger,
            mode: prefs.hotkey.mode,
            keys: None,
        };
        match HotkeyMonitor::start(binding, tx) {
            Ok(monitor) => {
                let adapter = monitor.kind();
                *inner.hotkey.lock() = Some(monitor);
                if let Some(monitor) = inner.hotkey.lock().as_ref() {
                    let (qa_trigger, translation_trigger) = modifier_shortcut_triggers(&inner);
                    monitor.update_modifier_shortcuts(qa_trigger, translation_trigger);
                }
                *inner.hotkey_status.lock() = HotkeyStatus {
                    adapter,
                    state: HotkeyStatusState::Installed,
                    message: Some(format!("{} 已安装", adapter.display_name())),
                    last_error: None,
                };
                log::info!(
                    "[coord] hotkey listener installed (after {} attempt(s))",
                    attempts + 1
                );
                let inner_clone = Arc::clone(&inner);
                std::thread::Builder::new()
                    .name("listener-type-hotkey-bridge".into())
                    .spawn(move || hotkey_bridge_loop(inner_clone, rx))
                    .ok();
                return;
            }
            Err(e) => {
                attempts += 1;
                let error_message = e.message.clone();
                *inner.hotkey_status.lock() = HotkeyStatus {
                    adapter: capability.adapter,
                    state: HotkeyStatusState::Failed,
                    message: Some(error_message.clone()),
                    last_error: Some(e),
                };
                if attempts <= 3 || attempts % 10 == 0 {
                    log::warn!(
                        "[coord] hotkey listener attempt #{attempts} failed: {}; retrying in 3s",
                        error_message
                    );
                }
                std::thread::sleep(std::time::Duration::from_secs(3));
            }
        }
    }
}

// ─────────────────────────── QA hotkey supervisor ───────────────────────────

fn qa_hotkey_supervisor_loop(inner: Arc<Inner>) {
    let mut attempts: u32 = 0;
    loop {
        if inner.shutdown.load(Ordering::SeqCst) {
            return;
        }
        // 用户已将 QA 关闭时会先进入待激活状态，prefs 改动通过 update_qa_hotkey_binding 唤醒逻辑恢复。
        let binding = match inner.prefs.get().qa_hotkey.clone() {
            Some(b) => b,
            None => {
                inner.qa_hotkey.lock().take();
                std::thread::sleep(std::time::Duration::from_secs(5));
                continue;
            }
        };
        if crate::shortcut_binding::legacy_modifier_trigger(&binding).is_some() {
            inner.qa_hotkey.lock().take();
            if let Some(monitor) = inner.hotkey.lock().as_ref() {
                let (qa_trigger, translation_trigger) = modifier_shortcut_triggers(&inner);
                monitor.update_modifier_shortcuts(qa_trigger, translation_trigger);
            }
            std::thread::sleep(std::time::Duration::from_secs(5));
            continue;
        }

        if inner.qa_hotkey.lock().is_some() {
            // 已注册成功 → 不重复装；睡 5s 复查（ binding 变化由 update 路径手动触发 ）。
            std::thread::sleep(std::time::Duration::from_secs(5));
            continue;
        }

        // global-hotkey crate 在 macOS 走 Carbon RegisterEventHotKey，要求 manager
        // 在主线程构造，否则 register() 看起来 Ok 但事件根本不会派发——这是 issue #118
        // PR #119 第一版漏掉的关键步骤，导致用户按了 hotkey 完全无反应。这里通过
        // run_on_main_thread 把 QaHotkeyMonitor::start 跳到主线程跑，结果再回 channel。
        let app = inner.app.lock().clone();
        let app = match app {
            Some(a) => a,
            None => {
                // 启动期 AppHandle 还没 bind，再等。
                std::thread::sleep(std::time::Duration::from_secs(1));
                continue;
            }
        };

        let (tx, rx) = mpsc::channel::<QaHotkeyEvent>();
        let (init_tx, init_rx) = mpsc::sync_channel::<Result<QaHotkeyMonitor, QaHotkeyError>>(1);
        let binding_for_main = binding.clone();
        let _ = app.run_on_main_thread(move || {
            let result = QaHotkeyMonitor::start(binding_for_main, tx);
            let _ = init_tx.send(result);
        });

        // run_on_main_thread 是 fire-and-forget；等主线程跑完结果回来。给 5s 上限避免
        // 主线程繁忙时 supervisor 永久阻塞。
        let init_result = match init_rx.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok(r) => r,
            Err(_) => {
                attempts += 1;
                if attempts <= 3 || attempts % 10 == 0 {
                    log::warn!(
                        "[coord] QA hotkey 第 {attempts} 次注册超时（主线程未回执）；3s 后重试"
                    );
                }
                std::thread::sleep(std::time::Duration::from_secs(3));
                continue;
            }
        };

        match init_result {
            Ok(monitor) => {
                *inner.qa_hotkey.lock() = Some(monitor);
                log::info!(
                    "[coord] QA hotkey listener installed on main thread (after {} attempt(s))",
                    attempts + 1
                );
                let inner_clone = Arc::clone(&inner);
                std::thread::Builder::new()
                    .name("listener-type-qa-hotkey-bridge".into())
                    .spawn(move || qa_hotkey_bridge_loop(inner_clone, rx))
                    .ok();
                attempts = 0;
            }
            Err(e) => {
                attempts += 1;
                if attempts <= 3 || attempts % 10 == 0 {
                    log::warn!("[coord] QA hotkey 第 {attempts} 次注册失败: {e}; 3s 后重试");
                }
                std::thread::sleep(std::time::Duration::from_secs(3));
            }
        }
    }
}

fn qa_hotkey_bridge_loop(inner: Arc<Inner>, rx: mpsc::Receiver<QaHotkeyEvent>) {
    while let Ok(evt) = rx.recv() {
        if inner.shortcut_recording_active.load(Ordering::SeqCst) {
            continue;
        }
        let inner_cloned = Arc::clone(&inner);
        match evt {
            QaHotkeyEvent::Pressed => {
                async_runtime::spawn(async move { handle_qa_hotkey_pressed(&inner_cloned).await });
            }
        }
    }
}

// ─────────────────────────── combo hotkey supervisor ───────────────────────────

fn combo_hotkey_supervisor_loop(inner: Arc<Inner>) {
    let mut attempts: u32 = 0;
    loop {
        if inner.shutdown.load(Ordering::SeqCst) {
            return;
        }
        // 读当前 prefs
        let prefs = inner.prefs.get();
        if crate::shortcut_binding::legacy_modifier_trigger(&prefs.dictation_hotkey).is_some() {
            // 不是 Custom → 待唤醒状态，等待 prefs 改动触发。
            take_combo_hotkey_on_main_thread(&inner);
            std::thread::sleep(std::time::Duration::from_secs(5));
            continue;
        }

        let binding = prefs.dictation_hotkey.clone();
        if is_unconfigured_shortcut(&binding) {
            take_combo_hotkey_on_main_thread(&inner);
            std::thread::sleep(std::time::Duration::from_secs(5));
            continue;
        }

        if inner.combo_hotkey.lock().is_some() {
            std::thread::sleep(std::time::Duration::from_secs(5));
            continue;
        }

        let app = inner.app.lock().clone();
        let app = match app {
            Some(a) => a,
            None => {
                std::thread::sleep(std::time::Duration::from_secs(1));
                continue;
            }
        };

        let (tx, rx) = mpsc::channel::<ComboHotkeyEvent>();
        let (init_tx, init_rx) =
            mpsc::sync_channel::<Result<ComboHotkeyMonitor, ComboHotkeyError>>(1);
        let binding_for_main = binding.clone();
        let _ = app.run_on_main_thread(move || {
            let result = ComboHotkeyMonitor::start(binding_for_main, tx);
            let _ = init_tx.send(result);
        });

        let init_result = match init_rx.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok(r) => r,
            Err(_) => {
                attempts += 1;
                if attempts <= 3 || attempts % 10 == 0 {
                    log::warn!(
                        "[coord] combo hotkey 第 {attempts} 次注册超时（主线程未回执）；3s 后重试"
                    );
                }
                std::thread::sleep(std::time::Duration::from_secs(3));
                continue;
            }
        };

        match init_result {
            Ok(monitor) => {
                *inner.combo_hotkey.lock() = Some(monitor);
                log::info!(
                    "[coord] combo hotkey listener installed on main thread (after {} attempt(s))",
                    attempts + 1
                );
                let inner_clone = Arc::clone(&inner);
                std::thread::Builder::new()
                    .name("listener-type-combo-hotkey-bridge".into())
                    .spawn(move || combo_hotkey_bridge_loop(inner_clone, rx))
                    .ok();
                attempts = 0;
            }
            Err(e) => {
                attempts += 1;
                if attempts <= 3 || attempts % 10 == 0 {
                    log::warn!("[coord] combo hotkey 第 {attempts} 次注册失败: {e}; 3s 后重试");
                }
                std::thread::sleep(std::time::Duration::from_secs(3));
            }
        }
    }
}

fn combo_hotkey_bridge_loop(inner: Arc<Inner>, rx: mpsc::Receiver<ComboHotkeyEvent>) {
    while let Ok(evt) = rx.recv() {
        if inner.shortcut_recording_active.load(Ordering::SeqCst) {
            continue;
        }
        let inner_cloned = Arc::clone(&inner);
        match evt {
            ComboHotkeyEvent::Pressed => {
                async_runtime::spawn(async move { handle_pressed_edge(&inner_cloned).await });
            }
            ComboHotkeyEvent::Released => {
                async_runtime::spawn(async move { handle_released_edge(&inner_cloned).await });
            }
        }
    }
}

fn translation_hotkey_supervisor_loop(inner: Arc<Inner>) {
    let mut attempts: u32 = 0;
    loop {
        if inner.shutdown.load(Ordering::SeqCst) {
            return;
        }
        let binding = inner.prefs.get().translation_hotkey;
        if is_builtin_translation_shift(&binding)
            || crate::shortcut_binding::legacy_modifier_trigger(&binding).is_some()
        {
            take_translation_hotkey_on_main_thread(&inner);
            if let Some(monitor) = inner.hotkey.lock().as_ref() {
                let (qa_trigger, translation_trigger) = modifier_shortcut_triggers(&inner);
                monitor.update_modifier_shortcuts(qa_trigger, translation_trigger);
            }
            std::thread::sleep(std::time::Duration::from_secs(5));
            continue;
        }

        if inner.translation_hotkey.lock().is_some() {
            std::thread::sleep(std::time::Duration::from_secs(5));
            continue;
        }

        let app = match inner.app.lock().clone() {
            Some(a) => a,
            None => {
                std::thread::sleep(std::time::Duration::from_secs(1));
                continue;
            }
        };

        let (tx, rx) = mpsc::channel::<ComboHotkeyEvent>();
        let (init_tx, init_rx) =
            mpsc::sync_channel::<Result<ComboHotkeyMonitor, ComboHotkeyError>>(1);
        let binding_for_main = binding.clone();
        let _ = app.run_on_main_thread(move || {
            let result = ComboHotkeyMonitor::start(binding_for_main, tx);
            let _ = init_tx.send(result);
        });

        let init_result = match init_rx.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok(r) => r,
            Err(_) => {
                attempts += 1;
                std::thread::sleep(std::time::Duration::from_secs(3));
                continue;
            }
        };

        match init_result {
            Ok(monitor) => {
                *inner.translation_hotkey.lock() = Some(monitor);
                let inner_clone = Arc::clone(&inner);
                std::thread::Builder::new()
                    .name("listener-type-translation-hotkey-bridge".into())
                    .spawn(move || translation_hotkey_bridge_loop(inner_clone, rx))
                    .ok();
                attempts = 0;
            }
            Err(e) => {
                attempts += 1;
                if attempts <= 3 || attempts % 10 == 0 {
                    log::warn!(
                        "[coord] translation hotkey 第 {attempts} 次注册失败: {e}; 3s 后重试"
                    );
                }
                std::thread::sleep(std::time::Duration::from_secs(3));
            }
        }
    }
}

fn update_translation_hotkey_on_main_thread(
    inner: Arc<Inner>,
    binding: crate::types::ShortcutBinding,
) -> Result<(), ComboHotkeyError> {
    if let Some(monitor) = inner.translation_hotkey.lock().as_ref() {
        return monitor.update_binding(binding);
    }
    let (tx, rx) = mpsc::channel::<ComboHotkeyEvent>();
    let monitor = ComboHotkeyMonitor::start(binding, tx)?;
    *inner.translation_hotkey.lock() = Some(monitor);
    let bridge_inner = Arc::clone(&inner);
    std::thread::Builder::new()
        .name("listener-type-translation-hotkey-bridge".into())
        .spawn(move || translation_hotkey_bridge_loop(bridge_inner, rx))
        .map_err(|e| ComboHotkeyError::RegisterFailed(format!("spawn bridge thread: {e}")))?;
    Ok(())
}

fn translation_hotkey_bridge_loop(inner: Arc<Inner>, rx: mpsc::Receiver<ComboHotkeyEvent>) {
    while let Ok(evt) = rx.recv() {
        if inner.shortcut_recording_active.load(Ordering::SeqCst) {
            continue;
        }
        if matches!(evt, ComboHotkeyEvent::Pressed) {
            mark_translation_modifier_seen(&inner);
        }
    }
}

fn action_hotkey_supervisor_loop(inner: Arc<Inner>, kind: ActionHotkeyKind) {
    let mut attempts: u32 = 0;
    loop {
        if inner.shutdown.load(Ordering::SeqCst) {
            return;
        }
        let binding = action_hotkey_binding(&inner, kind);
        if is_modifier_only_shortcut(&binding) {
            take_action_hotkey_on_main_thread(&inner, kind);
            std::thread::sleep(std::time::Duration::from_secs(5));
            continue;
        }

        if action_hotkey_slot(&inner, kind).lock().is_some() {
            std::thread::sleep(std::time::Duration::from_secs(5));
            continue;
        }

        let app = match inner.app.lock().clone() {
            Some(a) => a,
            None => {
                std::thread::sleep(std::time::Duration::from_secs(1));
                continue;
            }
        };

        let (tx, rx) = mpsc::channel::<ComboHotkeyEvent>();
        let (init_tx, init_rx) =
            mpsc::sync_channel::<Result<ComboHotkeyMonitor, ComboHotkeyError>>(1);
        let binding_for_main = binding.clone();
        let _ = app.run_on_main_thread(move || {
            let result = ComboHotkeyMonitor::start(binding_for_main, tx);
            let _ = init_tx.send(result);
        });

        let init_result = match init_rx.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok(r) => r,
            Err(_) => {
                attempts += 1;
                if attempts <= 3 || attempts % 10 == 0 {
                    log::warn!(
                        "[coord] action hotkey {kind:?} 第 {attempts} 次注册超时；3s 后重试"
                    );
                }
                std::thread::sleep(std::time::Duration::from_secs(3));
                continue;
            }
        };

        match init_result {
            Ok(monitor) => {
                *action_hotkey_slot(&inner, kind).lock() = Some(monitor);
                log::info!(
                    "[coord] action hotkey {kind:?} listener installed after {} attempt(s)",
                    attempts + 1
                );
                let inner_clone = Arc::clone(&inner);
                std::thread::Builder::new()
                    .name(action_hotkey_bridge_thread_name(kind).into())
                    .spawn(move || action_hotkey_bridge_loop(inner_clone, rx, kind))
                    .ok();
                attempts = 0;
            }
            Err(e) => {
                attempts += 1;
                if attempts <= 3 || attempts % 10 == 0 {
                    log::warn!(
                        "[coord] action hotkey {kind:?} 第 {attempts} 次注册失败: {e}; 3s 后重试"
                    );
                }
                std::thread::sleep(std::time::Duration::from_secs(3));
            }
        }
    }
}

fn action_hotkey_bridge_loop(
    inner: Arc<Inner>,
    rx: mpsc::Receiver<ComboHotkeyEvent>,
    kind: ActionHotkeyKind,
) {
    while let Ok(evt) = rx.recv() {
        if inner.shortcut_recording_active.load(Ordering::SeqCst) {
            crate::timeline::mark(
                "backend.hotkey",
                "ignored_shortcut_recording_active",
                format!("kind={kind:?} event={evt:?}"),
            );
            continue;
        }
        crate::timeline::mark(
            "backend.hotkey",
            "event",
            format!("kind={kind:?} event={evt:?}"),
        );
        if matches!(evt, ComboHotkeyEvent::Pressed) {
            handle_action_hotkey_pressed(&inner, kind);
        }
    }
}

fn handle_action_hotkey_pressed(inner: &Arc<Inner>, kind: ActionHotkeyKind) {
    match kind {
        ActionHotkeyKind::SwitchStyle => switch_to_previous_style(inner),
        ActionHotkeyKind::OpenApp => {
            if let Some(app) = inner.app.lock().clone() {
                let app_for_main = app.clone();
                let _ = app.run_on_main_thread(move || {
                    crate::show_main_window(&app_for_main);
                });
            }
        }
        ActionHotkeyKind::DeviceKey { key, gesture } => {
            handle_device_custom_key_pressed(inner, key, gesture)
        }
    }
}

fn handle_device_custom_key_pressed(
    inner: &Arc<Inner>,
    key: DeviceCustomKeyId,
    gesture: DeviceCustomKeyGesture,
) {
    let mapping = device_custom_key_mapping(inner, key, gesture);
    crate::timeline::mark(
        "backend.device_key",
        "pressed",
        format!(
            "key={} gesture={} action={:?}",
            key.label(),
            gesture.label(),
            mapping.action
        ),
    );
    if device_key_action_debounced(inner, key, gesture, &mapping) {
        return;
    }
    log::info!(
        "[device-key] {} {} pressed action={:?}",
        key.label(),
        gesture.label(),
        mapping.action
    );

    match mapping.action {
        DeviceCustomKeyAction::Disabled => {}
        DeviceCustomKeyAction::OpenApp => {
            if let Some(app) = inner.app.lock().clone() {
                let app_for_main = app.clone();
                let app_page = mapping.app_page;
                let _ = app.run_on_main_thread(move || {
                    crate::show_main_window(&app_for_main);
                    let _ = app_for_main.emit("device-key:open-app-page", app_page);
                });
                crate::timeline::mark(
                    "backend.device_key",
                    "open_app_page",
                    format!(
                        "key={} gesture={} page={:?}",
                        key.label(),
                        gesture.label(),
                        app_page
                    ),
                );
            }
        }
        DeviceCustomKeyAction::OpenExternalApp => {
            let path = mapping.external_app_path.trim();
            if path.is_empty() {
                log::warn!("[device-key] {} external app path is empty", key.label());
                emit_capsule(
                    inner,
                    CapsuleState::Error,
                    0.0,
                    0,
                    Some("设备键打开应用失败：路径为空".to_string()),
                    None,
                );
                schedule_capsule_idle(inner, 2200, None);
                return;
            }
            if let Err(error) = open_external_app_path(path) {
                log::warn!(
                    "[device-key] {} failed to open external app {path}: {error}",
                    key.label()
                );
                emit_capsule(
                    inner,
                    CapsuleState::Error,
                    0.0,
                    0,
                    Some("打开应用失败".to_string()),
                    None,
                );
                schedule_capsule_idle(inner, 3000, None);
            } else {
                crate::timeline::mark(
                    "backend.device_key",
                    "open_external_app",
                    format!(
                        "key={} gesture={} path={path}",
                        key.label(),
                        gesture.label()
                    ),
                );
                log::info!(
                    "[device-key] {} opened external app path={path}",
                    key.label()
                );
            }
        }
        DeviceCustomKeyAction::Dictation => {
            let inner = Arc::clone(inner);
            async_runtime::spawn(async move {
                handle_device_dictation_action(inner, key, gesture).await;
            });
        }
        DeviceCustomKeyAction::CopyShortcut => {
            send_builtin_shortcut(inner, key, gesture, "C", "copy");
        }
        DeviceCustomKeyAction::PasteShortcut => {
            send_builtin_shortcut(inner, key, gesture, "V", "paste");
        }
        DeviceCustomKeyAction::UndoShortcut => {
            send_builtin_shortcut(inner, key, gesture, "Z", "undo");
        }
        DeviceCustomKeyAction::SwitchStyle => switch_to_previous_style(inner),
        DeviceCustomKeyAction::SelectionAsk => {
            let inner = Arc::clone(inner);
            async_runtime::spawn(async move { handle_qa_hotkey_pressed(&inner).await });
        }
        DeviceCustomKeyAction::Translation => {
            let inner = Arc::clone(inner);
            async_runtime::spawn(async move { handle_device_translation_action(inner).await });
        }
        DeviceCustomKeyAction::PasteTemplate => {
            let text = mapping.paste_template.trim();
            if text.is_empty() {
                log::warn!("[device-key] {} paste template is empty", key.label());
                return;
            }
            let prefs = inner.prefs.get();
            let status = inner.inserter.insert(
                text,
                prefs.restore_clipboard_after_paste,
                prefs.paste_shortcut,
            );
            log::info!(
                "[device-key] {} pasted template chars={} status={:?}",
                key.label(),
                text.chars().count(),
                status
            );
        }
        DeviceCustomKeyAction::SendShortcut => {
            let Some(shortcut) = mapping.shortcut.as_ref() else {
                log::warn!(
                    "[device-key] {} shortcut action has no binding",
                    key.label()
                );
                return;
            };
            match crate::shortcut_dispatch::send_shortcut(shortcut) {
                Ok(()) => log::info!(
                    "[device-key] {} sent shortcut {}",
                    key.label(),
                    shortcut.display_label()
                ),
                Err(error) => log::warn!(
                    "[device-key] {} failed to send shortcut {}: {error}",
                    key.label(),
                    shortcut.display_label()
                ),
            }
        }
    }
}

fn device_custom_key_mapping(
    inner: &Arc<Inner>,
    key: DeviceCustomKeyId,
    gesture: DeviceCustomKeyGesture,
) -> DeviceCustomKeyMapping {
    let prefs = inner.prefs.get();
    match gesture {
        DeviceCustomKeyGesture::SingleClick => prefs.device_custom_keys.get(key).clone(),
        DeviceCustomKeyGesture::DoubleClick => {
            prefs.device_custom_key_double_clicks.get(key).clone()
        }
        DeviceCustomKeyGesture::LongPress => prefs.device_custom_key_long_presses.get(key).clone(),
    }
}

fn device_key_action_debounced(
    inner: &Arc<Inner>,
    key: DeviceCustomKeyId,
    gesture: DeviceCustomKeyGesture,
    mapping: &DeviceCustomKeyMapping,
) -> bool {
    let window = match mapping.action {
        DeviceCustomKeyAction::Disabled => return false,
        DeviceCustomKeyAction::CopyShortcut
        | DeviceCustomKeyAction::PasteShortcut
        | DeviceCustomKeyAction::UndoShortcut
        | DeviceCustomKeyAction::SendShortcut => Duration::from_millis(160),
        DeviceCustomKeyAction::Dictation => HOTKEY_DEBOUNCE,
        DeviceCustomKeyAction::OpenApp | DeviceCustomKeyAction::OpenExternalApp => {
            Duration::from_millis(900)
        }
        _ => Duration::from_millis(350),
    };
    let now = Instant::now();
    let mut last_dispatch = inner.device_key_last_dispatch_at.lock();
    let key_tuple = (gesture, key);
    if let Some(last) = last_dispatch.get(&key_tuple) {
        if now.duration_since(*last) < window {
            crate::timeline::mark(
                "backend.device_key",
                "debounced",
                format!(
                    "key={} gesture={} action={:?} window_ms={}",
                    key.label(),
                    gesture.label(),
                    mapping.action,
                    window.as_millis()
                ),
            );
            return true;
        }
    }
    last_dispatch.insert(key_tuple, now);
    false
}

fn builtin_shortcut(primary: &str) -> ShortcutBinding {
    ShortcutBinding {
        primary: primary.into(),
        modifiers: vec![if cfg!(target_os = "macos") {
            "cmd".into()
        } else {
            "ctrl".into()
        }],
    }
}

fn send_builtin_shortcut(
    inner: &Arc<Inner>,
    key: DeviceCustomKeyId,
    gesture: DeviceCustomKeyGesture,
    primary: &str,
    label: &str,
) {
    let shortcut = builtin_shortcut(primary);
    crate::timeline::mark(
        "backend.device_key",
        "send_builtin_shortcut",
        format!(
            "key={} gesture={} label={} binding={}",
            key.label(),
            gesture.label(),
            label,
            shortcut.display_label()
        ),
    );
    match crate::shortcut_dispatch::send_shortcut(&shortcut) {
        Ok(()) => log::info!("[device-key] {} sent {label}", key.label()),
        Err(error) => {
            log::warn!(
                "[device-key] {} failed to send {label}: {error}",
                key.label()
            );
            emit_capsule(
                inner,
                CapsuleState::Error,
                0.0,
                0,
                Some(format!("{label} 发送失败")),
                None,
            );
            schedule_capsule_idle(inner, 2200, None);
        }
    }
}

#[cfg(target_os = "windows")]
fn open_external_app_path(path: &str) -> Result<(), String> {
    let path = validate_external_app_path(path)?;
    shell_execute_open(&path)
}

#[cfg(target_os = "macos")]
fn open_external_app_path(path: &str) -> Result<(), String> {
    let path = validate_external_app_path(path)?;
    std::process::Command::new("/usr/bin/open")
        .arg(&path)
        .spawn()
        .map(|_| ())
        .map_err(|err| err.to_string())
}

#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
fn open_external_app_path(path: &str) -> Result<(), String> {
    let path = validate_external_app_path(path)?;
    if path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("desktop"))
    {
        return std::process::Command::new("xdg-open")
            .arg(&path)
            .spawn()
            .map(|_| ())
            .map_err(|err| err.to_string());
    }
    std::process::Command::new(&path)
        .spawn()
        .map(|_| ())
        .map_err(|err| err.to_string())
}

fn validate_external_app_path(path: &str) -> Result<PathBuf, String> {
    let path = path.trim();
    if path.is_empty() {
        return Err("应用路径为空".into());
    }
    let path = PathBuf::from(path);
    if !path.exists() {
        return Err("应用路径不存在，请从已安装应用中选择或填写完整应用路径".into());
    }
    if !is_supported_external_app_path(&path) {
        return Err("仅支持已安装应用或应用快捷方式路径，不支持命令或脚本".into());
    }
    Ok(path)
}

#[cfg(target_os = "windows")]
fn is_supported_external_app_path(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|ext| {
                matches!(
                    ext.to_ascii_lowercase().as_str(),
                    "exe" | "lnk" | "appref-ms"
                )
            })
}

#[cfg(target_os = "macos")]
fn is_supported_external_app_path(path: &Path) -> bool {
    path.is_dir()
        && path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("app"))
}

#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
fn is_supported_external_app_path(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("desktop"))
}

#[cfg(target_os = "windows")]
fn shell_execute_open(path: &Path) -> Result<(), String> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    fn wide(value: &OsStr) -> Vec<u16> {
        value.encode_wide().chain(Some(0)).collect()
    }

    let operation = wide(OsStr::new("open"));
    let file = wide(path.as_os_str());
    let result = unsafe {
        ShellExecuteW(
            HWND(std::ptr::null_mut()),
            PCWSTR(operation.as_ptr()),
            PCWSTR(file.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    if result.0 as isize > 32 {
        Ok(())
    } else {
        Err(format!("ShellExecuteW failed code={}", result.0 as isize))
    }
}

async fn handle_device_dictation_action(
    inner: Arc<Inner>,
    key: DeviceCustomKeyId,
    gesture: DeviceCustomKeyGesture,
) {
    if !recording_gate::try_admit_arc(
        &inner,
        recording_gate::RecordIntent::DeviceKeyDictation,
        &format!(
            "device-key dictation key={} gesture={}",
            key.label(),
            gesture.label()
        ),
    ) {
        crate::timeline::mark(
            "backend.device_key",
            "dictation_action_blocked_ota",
            format!("key={} gesture={}", key.label(), gesture.label()),
        );
        return;
    }
    let input_source = inner.prefs.get().dictation_input_source;
    let phase = inner.state.lock().phase;
    let wake_started_at = (!embedded_ble_listener_capture_ready(&inner)).then(Instant::now);
    crate::timeline::mark(
        "backend.device_key",
        "dictation_action",
        format!(
            "key={} gesture={} source={input_source:?} phase={phase:?}",
            key.label(),
            gesture.label()
        ),
    );

    if input_source == DictationInputSource::EmbeddedBle {
        let mut queue_pending_start_on_wait_failure = false;
        if !embedded_ble_listener_capture_ready(&inner) {
            if matches!(
                device_key_ble_recording_control_decision(&inner),
                DeviceKeyBleRecordingControlDecision::Start
            ) {
                queue_pending_start_on_wait_failure = true;
            }
            record_embedded_ble_reconnect_attempt(&inner, "device_key_recording_control");
            refresh_embedded_ble_listener_for_device_key_wake(&inner);
            emit_capsule(
                &inner,
                CapsuleState::Reconnecting,
                0.0,
                0,
                Some("正在恢复 Listener 音频".to_string()),
                None,
            );
        }
        if let Err(error) = wait_for_embedded_ble_listener_ready(
            &inner,
            EMBEDDED_BLE_RECORDING_CONTROL_READY_TIMEOUT,
        )
        .await
        {
            record_embedded_ble_listener_last_error(&inner, &error);
            record_embedded_ble_recovery_failure(&inner, &error);
            crate::timeline::mark(
                "backend.device_key",
                "ble_recording_control_wait_failed",
                format!(
                    "key={} gesture={} queue_pending_start={} error={error}",
                    key.label(),
                    gesture.label(),
                    queue_pending_start_on_wait_failure
                ),
            );
            if queue_pending_start_on_wait_failure {
                queue_pending_device_key_ble_start(&inner, key, gesture, "listener_not_ready");
                emit_capsule(
                    &inner,
                    CapsuleState::Reconnecting,
                    0.0,
                    0,
                    Some("Listener 恢复后自动录音".to_string()),
                    None,
                );
                return;
            }
            emit_capsule(
                &inner,
                CapsuleState::Error,
                0.0,
                0,
                Some("Listener 音频通道未恢复".to_string()),
                None,
            );
            schedule_capsule_idle(&inner, 6000, None);
            return;
        }
        if let Some(started_at) = wake_started_at {
            let elapsed_ms = started_at.elapsed().as_millis();
            let target_ms = EMBEDDED_BLE_IDLE_AUDIO_WAKE_TARGET.as_millis();
            crate::timeline::mark(
                "backend.device_key",
                "idle_audio_notify_ready",
                format!(
                    "key={} gesture={} elapsed_ms={elapsed_ms} target_ms={target_ms} met={}",
                    key.label(),
                    gesture.label(),
                    elapsed_ms <= target_ms,
                ),
            );
            log::info!(
                "[embedded-ble] device-key Idle audio notify ready elapsed_ms={elapsed_ms} target_ms={target_ms} met={}",
                elapsed_ms <= target_ms,
            );
        }

        let control_decision = device_key_ble_recording_control_decision(&inner);
        let promote_hidden_candidate = should_promote_hidden_automatic_candidate(
            control_decision,
            hidden_automatic_candidate_active(),
        );
        if let DeviceKeyBleRecordingControlDecision::IgnoreStarting {
            session_id,
            elapsed_ms,
        } = control_decision
        {
            crate::timeline::mark(
                "backend.device_key",
                "ble_recording_control_ignored_starting",
                format!(
                    "key={} gesture={} session_id={session_id} elapsed_ms={elapsed_ms}",
                    key.label(),
                    gesture.label()
                ),
            );
            log::info!(
                "[device-key] {} {} recording control ignored while embedded BLE session is starting elapsed_ms={elapsed_ms}",
                key.label(),
                gesture.label()
            );
            emit_device_key_recording_control_capsule(
                &inner,
                Some((session_id, SessionPhase::Starting)),
                DictationUiState::Recording,
                CapsuleState::Reconnecting,
                "正在等待 Listener 音频...".to_string(),
            );
            return;
        }

        let control_session = control_decision.control_session();
        let waiting_message = if promote_hidden_candidate {
            "正在接管当前录音..."
        } else if control_session.is_some() {
            "正在发送设备录音停止控制..."
        } else {
            "正在启动 Listener 录音..."
        };

        let stop_feedback_requested =
            if matches!(control_session, Some((_, SessionPhase::Listening))) {
                request_embedded_audio_stop_feedback(&inner, "device_key_stop_control_pending")
            } else {
                false
            };
        let pending_ui_state = if stop_feedback_requested {
            DictationUiState::Transcribing
        } else {
            DictationUiState::Recording
        };
        let waiting_message = if stop_feedback_requested {
            current_embedded_audio_partial_preview(&inner)
                .unwrap_or_else(|| waiting_message.to_string())
        } else {
            waiting_message.to_string()
        };
        emit_device_key_recording_control_capsule(
            &inner,
            control_session,
            pending_ui_state,
            if control_session.is_some() {
                CapsuleState::Reconnecting
            } else {
                CapsuleState::Recording
            },
            waiting_message,
        );
        let send_stop_control = matches!(
            control_decision,
            DeviceKeyBleRecordingControlDecision::Stop {
                phase: SessionPhase::Listening,
                ..
            }
        );
        let result = async_runtime::spawn_blocking(move || {
            if promote_hidden_candidate {
                crate::embedded_ble::send_recording_control_activate(
                    EMBEDDED_BLE_RECORDING_CONTROL_WRITE_TIMEOUT,
                )
            } else if send_stop_control {
                crate::embedded_ble::send_recording_control_stop(
                    EMBEDDED_BLE_RECORDING_CONTROL_WRITE_TIMEOUT,
                )
            } else {
                crate::embedded_ble::send_recording_control_toggle(
                    EMBEDDED_BLE_RECORDING_CONTROL_WRITE_TIMEOUT,
                )
            }
        })
        .await
        .map_err(|err| err.to_string())
        .and_then(|value| value);
        match result {
            Ok(()) => {
                let promotion_requested = if promote_hidden_candidate {
                    request_hidden_automatic_candidate_promotion()
                } else {
                    false
                };
                clear_embedded_ble_listener_last_error(&inner);
                clear_pending_device_key_ble_start(&inner, key, gesture, "control_sent");
                crate::timeline::mark(
                    "backend.device_key",
                    "ble_recording_control_sent",
                    format!(
                        "key={} gesture={} hidden_candidate_promotion={promotion_requested}",
                        key.label(),
                        gesture.label()
                    ),
                );
                if promote_hidden_candidate {
                    if promotion_requested {
                        log::info!(
                            "[device-key] {} {} promoted the active hidden automatic candidate",
                            key.label(),
                            gesture.label()
                        );
                    } else {
                        log::warn!(
                            "[device-key] {} {} activation arrived after the hidden automatic candidate had already resolved",
                            key.label(),
                            gesture.label()
                        );
                    }
                }
                if matches!(control_session, Some((_, SessionPhase::Listening))) {
                    return;
                }
                emit_device_key_recording_control_capsule(
                    &inner,
                    control_session,
                    DictationUiState::Recording,
                    CapsuleState::Recording,
                    "Listener 录音已启动，正在接收音频...".to_string(),
                );
            }
            Err(error) => {
                record_embedded_ble_listener_last_error(&inner, &error);
                record_embedded_ble_recovery_failure(&inner, &error);
                refresh_embedded_ble_listener(&inner);
                crate::timeline::mark(
                    "backend.device_key",
                    "ble_recording_control_failed",
                    format!(
                        "key={} gesture={} error={error}",
                        key.label(),
                        gesture.label()
                    ),
                );
                if let Some(kind) =
                    should_keep_device_key_ble_action_pending_after_error(control_decision, &error)
                {
                    match kind {
                        PendingDeviceKeyBleActionKind::Start => {
                            queue_pending_device_key_ble_start(
                                &inner,
                                key,
                                gesture,
                                "control_write_retryable_failure",
                            );
                            emit_capsule(
                                &inner,
                                CapsuleState::Reconnecting,
                                0.0,
                                0,
                                Some("Listener 恢复后自动录音".to_string()),
                                None,
                            );
                        }
                        PendingDeviceKeyBleActionKind::Stop => {
                            queue_pending_device_key_ble_stop(
                                &inner,
                                key,
                                gesture,
                                "control_write_retryable_failure",
                            );
                            emit_device_key_recording_control_capsule(
                                &inner,
                                control_session,
                                DictationUiState::Transcribing,
                                CapsuleState::Reconnecting,
                                "Listener 恢复后自动停止录音".to_string(),
                            );
                        }
                    }
                    return;
                }
                let idle_session = emit_device_key_recording_control_capsule(
                    &inner,
                    control_session,
                    DictationUiState::Error,
                    CapsuleState::Error,
                    "Listener 录音控制失败".to_string(),
                );
                schedule_capsule_idle(&inner, 6000, idle_session);
            }
        }
        return;
    }

    match phase {
        SessionPhase::Idle => {
            let _ = begin_session(&inner).await;
        }
        SessionPhase::Listening => {
            let _ = end_session(&inner).await;
        }
        SessionPhase::Starting => {
            request_stop_during_starting(&inner, "device key dictation toggle");
        }
        _ => {}
    }
}

fn device_key_ble_recording_control_decision(
    inner: &Arc<Inner>,
) -> DeviceKeyBleRecordingControlDecision {
    let state = inner.state.lock();
    match state.phase {
        SessionPhase::Starting => DeviceKeyBleRecordingControlDecision::IgnoreStarting {
            session_id: state.session_id,
            elapsed_ms: state.started_at.elapsed().as_millis() as u64,
        },
        SessionPhase::Listening => DeviceKeyBleRecordingControlDecision::Stop {
            session_id: state.session_id,
            phase: state.phase,
        },
        _ => DeviceKeyBleRecordingControlDecision::Start,
    }
}

fn should_promote_hidden_automatic_candidate(
    decision: DeviceKeyBleRecordingControlDecision,
    hidden_candidate_active: bool,
) -> bool {
    hidden_candidate_active && matches!(decision, DeviceKeyBleRecordingControlDecision::Start)
}

fn pending_device_key_ble_action_age(action: PendingDeviceKeyBleAction, now: Instant) -> Duration {
    now.checked_duration_since(action.queued_at)
        .unwrap_or_default()
}

fn pending_device_key_ble_action_is_fresh(action: PendingDeviceKeyBleAction, now: Instant) -> bool {
    pending_device_key_ble_action_age(action, now) <= DEVICE_KEY_BLE_PENDING_ACTION_TTL
}

fn queue_pending_device_key_ble_action(
    inner: &Arc<Inner>,
    kind: PendingDeviceKeyBleActionKind,
    key: DeviceCustomKeyId,
    gesture: DeviceCustomKeyGesture,
    reason: &'static str,
) {
    let action = PendingDeviceKeyBleAction {
        kind,
        key,
        gesture,
        queued_at: Instant::now(),
    };
    let previous = {
        let mut slot = inner.device_key_pending_ble_action.lock();
        slot.replace(action)
    };
    crate::timeline::mark(
        "backend.device_key",
        "ble_recording_control_pending_queued",
        format!(
            "kind={} key={} gesture={} reason={reason} replaced={}",
            kind.label(),
            key.label(),
            gesture.label(),
            previous.is_some()
        ),
    );
    log::info!(
        "[device-key] queued pending BLE recording {} key={} gesture={} reason={reason} replaced={}",
        kind.label(),
        key.label(),
        gesture.label(),
        previous.is_some()
    );
    schedule_pending_device_key_ble_action_expiry(inner, action);
}

fn take_expired_pending_device_key_ble_action(
    inner: &Arc<Inner>,
    expected: PendingDeviceKeyBleAction,
    reason: &'static str,
) -> Option<PendingDeviceKeyBleAction> {
    let expired = {
        let mut slot = inner.device_key_pending_ble_action.lock();
        if slot.as_ref().is_some_and(|action| {
            *action == expected && !pending_device_key_ble_action_is_fresh(*action, Instant::now())
        }) {
            slot.take()
        } else {
            None
        }
    };
    let Some(action) = expired else {
        return None;
    };
    let age_ms = pending_device_key_ble_action_age(action, Instant::now()).as_millis();
    crate::timeline::mark(
        "backend.device_key",
        "ble_recording_control_pending_expired",
        format!(
            "kind={} key={} gesture={} reason={reason} age_ms={age_ms}",
            action.kind.label(),
            action.key.label(),
            action.gesture.label()
        ),
    );
    log::warn!(
        "[device-key] pending BLE recording {} reached its terminal TTL key={} gesture={} reason={reason} age_ms={age_ms}",
        action.kind.label(),
        action.key.label(),
        action.gesture.label()
    );
    Some(action)
}

fn schedule_pending_device_key_ble_action_expiry(
    inner: &Arc<Inner>,
    action: PendingDeviceKeyBleAction,
) {
    let inner = Arc::clone(inner);
    async_runtime::spawn(async move {
        tokio::time::sleep(DEVICE_KEY_BLE_PENDING_ACTION_TTL + Duration::from_millis(1)).await;
        let Some(expired) =
            take_expired_pending_device_key_ble_action(&inner, action, "terminal_ttl")
        else {
            return;
        };
        if expired.kind == PendingDeviceKeyBleActionKind::Start
            && inner.state.lock().phase == SessionPhase::Idle
        {
            let idle_session = emit_device_key_recording_control_capsule(
                &inner,
                None,
                DictationUiState::Error,
                CapsuleState::Error,
                "Listener 音频通道未恢复".to_string(),
            );
            schedule_capsule_idle(&inner, 6000, idle_session);
        }
    });
}

fn queue_pending_device_key_ble_start(
    inner: &Arc<Inner>,
    key: DeviceCustomKeyId,
    gesture: DeviceCustomKeyGesture,
    reason: &'static str,
) {
    queue_pending_device_key_ble_action(
        inner,
        PendingDeviceKeyBleActionKind::Start,
        key,
        gesture,
        reason,
    );
}

fn queue_pending_device_key_ble_stop(
    inner: &Arc<Inner>,
    key: DeviceCustomKeyId,
    gesture: DeviceCustomKeyGesture,
    reason: &'static str,
) {
    queue_pending_device_key_ble_action(
        inner,
        PendingDeviceKeyBleActionKind::Stop,
        key,
        gesture,
        reason,
    );
}

fn clear_pending_device_key_ble_start(
    inner: &Arc<Inner>,
    key: DeviceCustomKeyId,
    gesture: DeviceCustomKeyGesture,
    reason: &'static str,
) -> bool {
    let removed = {
        let mut slot = inner.device_key_pending_ble_action.lock();
        if slot.as_ref().is_some_and(|action| {
            action.kind == PendingDeviceKeyBleActionKind::Start
                && action.key == key
                && action.gesture == gesture
        }) {
            slot.take()
        } else {
            None
        }
    };
    if let Some(action) = removed {
        let age_ms = pending_device_key_ble_action_age(action, Instant::now()).as_millis();
        crate::timeline::mark(
            "backend.device_key",
            "ble_recording_control_pending_cleared",
            format!(
                "key={} gesture={} reason={reason} age_ms={age_ms}",
                key.label(),
                gesture.label()
            ),
        );
        log::info!(
            "[device-key] cleared pending BLE recording start key={} gesture={} reason={reason} age_ms={age_ms}",
            key.label(),
            gesture.label()
        );
        true
    } else {
        false
    }
}

fn take_pending_device_key_ble_action(
    inner: &Arc<Inner>,
    reason: &'static str,
) -> Option<PendingDeviceKeyBleAction> {
    let action = inner.device_key_pending_ble_action.lock().take()?;
    let now = Instant::now();
    let age = pending_device_key_ble_action_age(action, now);
    let age_ms = age.as_millis();
    if !pending_device_key_ble_action_is_fresh(action, now) {
        crate::timeline::mark(
            "backend.device_key",
            "ble_recording_control_pending_expired",
            format!(
                "kind={} key={} gesture={} reason={reason} age_ms={age_ms}",
                action.kind.label(),
                action.key.label(),
                action.gesture.label()
            ),
        );
        log::info!(
            "[device-key] expired pending BLE recording {} key={} gesture={} reason={reason} age_ms={age_ms}",
            action.kind.label(),
            action.key.label(),
            action.gesture.label()
        );
        return None;
    }
    crate::timeline::mark(
        "backend.device_key",
        "ble_recording_control_pending_taken",
        format!(
            "kind={} key={} gesture={} reason={reason} age_ms={age_ms}",
            action.kind.label(),
            action.key.label(),
            action.gesture.label()
        ),
    );
    Some(action)
}

fn take_pending_device_key_ble_start(
    inner: &Arc<Inner>,
    reason: &'static str,
) -> Option<PendingDeviceKeyBleAction> {
    let action = take_pending_device_key_ble_action(inner, reason)?;
    if action.kind == PendingDeviceKeyBleActionKind::Start {
        Some(action)
    } else {
        let mut slot = inner.device_key_pending_ble_action.lock();
        *slot = Some(action);
        None
    }
}

fn drop_pending_device_key_ble_action_for_state(
    action: PendingDeviceKeyBleAction,
    decision: DeviceKeyBleRecordingControlDecision,
    reason: &'static str,
) {
    let age_ms = pending_device_key_ble_action_age(action, Instant::now()).as_millis();
    crate::timeline::mark(
        "backend.device_key",
        "ble_recording_control_pending_dropped_state_changed",
        format!(
            "kind={} key={} gesture={} reason={reason} decision={decision:?} age_ms={age_ms}",
            action.kind.label(),
            action.key.label(),
            action.gesture.label()
        ),
    );
    log::info!(
        "[device-key] pending BLE recording {} dropped because state changed key={} gesture={} reason={reason} decision={decision:?} age_ms={age_ms}",
        action.kind.label(),
        action.key.label(),
        action.gesture.label()
    );
}

fn should_keep_device_key_ble_action_pending_after_error(
    decision: DeviceKeyBleRecordingControlDecision,
    error: &str,
) -> Option<PendingDeviceKeyBleActionKind> {
    if !crate::embedded_ble::classify_ble_failure(error).automatic_recovery {
        return None;
    }
    match decision {
        DeviceKeyBleRecordingControlDecision::Start => Some(PendingDeviceKeyBleActionKind::Start),
        DeviceKeyBleRecordingControlDecision::Stop {
            phase: SessionPhase::Listening,
            ..
        } => Some(PendingDeviceKeyBleActionKind::Stop),
        _ => None,
    }
}

fn should_keep_device_key_ble_start_pending_after_error(
    decision: DeviceKeyBleRecordingControlDecision,
    error: &str,
) -> bool {
    should_keep_device_key_ble_action_pending_after_error(decision, error)
        == Some(PendingDeviceKeyBleActionKind::Start)
}

fn restore_pending_device_key_ble_action(
    inner: &Arc<Inner>,
    action: PendingDeviceKeyBleAction,
    reason: &'static str,
) {
    let previous = {
        let mut slot = inner.device_key_pending_ble_action.lock();
        slot.replace(action)
    };
    log::info!(
        "[device-key] pending BLE recording {} restored reason={reason} replaced={} age_ms={}",
        action.kind.label(),
        previous.is_some(),
        pending_device_key_ble_action_age(action, Instant::now()).as_millis()
    );
}

fn flush_pending_device_key_ble_start_action(
    inner: &Arc<Inner>,
    action: PendingDeviceKeyBleAction,
    reason: &'static str,
) {
    let decision = device_key_ble_recording_control_decision(inner);
    if !matches!(decision, DeviceKeyBleRecordingControlDecision::Start) {
        drop_pending_device_key_ble_action_for_state(action, decision, reason);
        return;
    }
    if !embedded_ble_listener_capture_ready(inner) {
        restore_pending_device_key_ble_action(inner, action, reason);
        log::info!(
            "[device-key] pending BLE recording start restored because notify is no longer ready reason={reason}"
        );
        return;
    }

    let inner = Arc::clone(inner);
    async_runtime::spawn(async move {
        handle_device_dictation_action(inner, action.key, action.gesture).await;
    });
}

fn flush_pending_device_key_ble_start(inner: &Arc<Inner>, reason: &'static str) {
    let Some(action) = take_pending_device_key_ble_start(inner, reason) else {
        return;
    };
    flush_pending_device_key_ble_start_action(inner, action, reason);
}

fn flush_pending_device_key_ble_stop_action(
    inner: &Arc<Inner>,
    action: PendingDeviceKeyBleAction,
    reason: &'static str,
) {
    let decision = device_key_ble_recording_control_decision(inner);
    if !matches!(
        decision,
        DeviceKeyBleRecordingControlDecision::Stop {
            phase: SessionPhase::Listening,
            ..
        }
    ) {
        drop_pending_device_key_ble_action_for_state(action, decision, reason);
        return;
    }
    if !embedded_ble_listener_capture_ready(inner) {
        restore_pending_device_key_ble_action(inner, action, reason);
        log::info!(
            "[device-key] pending BLE recording stop restored because notify is no longer ready reason={reason}"
        );
        return;
    }

    let inner = Arc::clone(inner);
    async_runtime::spawn(async move {
        send_pending_device_key_ble_stop(inner, action, reason).await;
    });
}

fn flush_pending_device_key_ble_action(inner: &Arc<Inner>, reason: &'static str) {
    let Some(action) = take_pending_device_key_ble_action(inner, reason) else {
        return;
    };
    match action.kind {
        PendingDeviceKeyBleActionKind::Start => {
            flush_pending_device_key_ble_start_action(inner, action, reason);
        }
        PendingDeviceKeyBleActionKind::Stop => {
            flush_pending_device_key_ble_stop_action(inner, action, reason);
        }
    }
}

async fn send_pending_device_key_ble_stop(
    inner: Arc<Inner>,
    action: PendingDeviceKeyBleAction,
    reason: &'static str,
) {
    let control_decision = device_key_ble_recording_control_decision(&inner);
    let control_session = control_decision.control_session();
    let Some((_, SessionPhase::Listening)) = control_session else {
        drop_pending_device_key_ble_action_for_state(action, control_decision, reason);
        return;
    };

    let _ = request_embedded_audio_stop_feedback(&inner, "device_key_stop_control_retry");
    emit_device_key_recording_control_capsule(
        &inner,
        control_session,
        DictationUiState::Transcribing,
        CapsuleState::Reconnecting,
        "正在补发设备录音停止控制...".to_string(),
    );

    let result = async_runtime::spawn_blocking(move || {
        crate::embedded_ble::send_recording_control_stop(
            EMBEDDED_BLE_RECORDING_CONTROL_WRITE_TIMEOUT,
        )
    })
    .await
    .map_err(|err| err.to_string())
    .and_then(|value| value);

    match result {
        Ok(()) => {
            clear_embedded_ble_listener_last_error(&inner);
            crate::timeline::mark(
                "backend.device_key",
                "ble_recording_control_pending_stop_sent",
                format!(
                    "key={} gesture={} reason={reason}",
                    action.key.label(),
                    action.gesture.label()
                ),
            );
            log::info!(
                "[device-key] pending BLE recording stop sent key={} gesture={} reason={reason}",
                action.key.label(),
                action.gesture.label()
            );
        }
        Err(error) => {
            record_embedded_ble_listener_last_error(&inner, &error);
            record_embedded_ble_recovery_failure(&inner, &error);
            refresh_embedded_ble_listener(&inner);
            crate::timeline::mark(
                "backend.device_key",
                "ble_recording_control_pending_stop_failed",
                format!(
                    "key={} gesture={} reason={reason} error={error}",
                    action.key.label(),
                    action.gesture.label()
                ),
            );
            if should_keep_device_key_ble_action_pending_after_error(control_decision, &error)
                == Some(PendingDeviceKeyBleActionKind::Stop)
                && pending_device_key_ble_action_is_fresh(action, Instant::now())
            {
                restore_pending_device_key_ble_action(&inner, action, "stop_retryable_failure");
                emit_device_key_recording_control_capsule(
                    &inner,
                    control_session,
                    DictationUiState::Transcribing,
                    CapsuleState::Reconnecting,
                    "Listener 恢复后自动停止录音".to_string(),
                );
                return;
            }
            let idle_session = emit_device_key_recording_control_capsule(
                &inner,
                control_session,
                DictationUiState::Error,
                CapsuleState::Error,
                "Listener 录音控制失败".to_string(),
            );
            schedule_capsule_idle(&inner, 6000, idle_session);
        }
    }
}

fn current_device_key_recording_control_session(
    inner: &Arc<Inner>,
) -> Option<(SessionId, SessionPhase)> {
    device_key_ble_recording_control_decision(inner).control_session()
}

fn emit_device_key_recording_control_capsule(
    inner: &Arc<Inner>,
    session: Option<(SessionId, SessionPhase)>,
    ui_state: DictationUiState,
    fallback_state: CapsuleState,
    message: String,
) -> Option<SessionId> {
    if let Some((session_id, _)) = session {
        if publish_dictation_capsule(
            inner,
            session_id,
            ui_state,
            0.0,
            Some(message.clone()),
            None,
        ) {
            return Some(session_id);
        }
        emit_capsule_for_session(
            inner,
            session_id,
            fallback_state,
            0.0,
            0,
            Some(message),
            None,
        );
        return Some(session_id);
    }

    emit_capsule(inner, fallback_state, 0.0, 0, Some(message), None);
    None
}

async fn request_embedded_ble_recording_start_from_host(
    inner: &Arc<Inner>,
    reason: &'static str,
) -> Result<SessionId, String> {
    if !embedded_ble_listener_capture_ready(inner) {
        return Err("Listener BLE audio channel is not ready".to_string());
    }

    let session_id = {
        let mut state = inner.state.lock();
        match state.phase {
            SessionPhase::Idle => begin_session_state(&mut state, None, capture_frontmost_app())
                .ok_or_else(|| "Listener BLE recording start ignored while idle".to_string())?,
            SessionPhase::Starting | SessionPhase::Listening => state.session_id,
            phase => {
                return Err(format!(
                    "Listener BLE recording start ignored while dictation phase is {phase:?}"
                ));
            }
        }
    };

    record_embedded_ble_session_actor_command(
        inner,
        EmbeddedBleSessionActorCommand::StartCommand,
        Some(session_id),
        format!("host start requested reason={reason}"),
    );
    emit_capsule_for_session(
        inner,
        session_id,
        CapsuleState::Recording,
        0.0,
        0,
        Some("Listener 录音已启动，正在接收音频...".to_string()),
        None,
    );

    #[cfg(test)]
    {
        crate::timeline::mark(
            "backend.embedded_ble_session_actor",
            "firmware_start_skipped_test",
            format!("session_id={session_id} reason={reason}"),
        );
        return Ok(session_id);
    }

    #[cfg(not(test))]
    {
        let result = async_runtime::spawn_blocking(move || {
            crate::embedded_ble::send_recording_control_toggle(
                EMBEDDED_BLE_RECORDING_CONTROL_WRITE_TIMEOUT,
            )
        })
        .await
        .map_err(|err| err.to_string())
        .and_then(|value| value);

        match result {
            Ok(()) => {
                crate::timeline::mark(
                    "backend.embedded_ble_session_actor",
                    "firmware_start_sent",
                    format!("session_id={session_id} reason={reason}"),
                );
                log::info!(
                    "[coord] embedded BLE firmware start sent session_id={session_id} reason={reason}"
                );
                Ok(session_id)
            }
            Err(err) => {
                set_phase_idle_if_session_matches(inner, session_id);
                record_embedded_ble_listener_last_error(inner, &err);
                record_embedded_ble_recovery_failure(inner, &err);
                refresh_embedded_ble_listener(inner);
                emit_capsule(
                    inner,
                    CapsuleState::Error,
                    0.0,
                    0,
                    Some("Listener 录音启动失败".to_string()),
                    None,
                );
                schedule_capsule_idle(inner, 6000, Some(session_id));
                Err(err)
            }
        }
    }
}

async fn handle_device_translation_action(inner: Arc<Inner>) {
    let phase = inner.state.lock().phase;
    if matches!(phase, SessionPhase::Idle) {
        let _ = begin_session(&inner).await;
        mark_translation_modifier_seen(&inner);
        return;
    }
    mark_translation_modifier_seen(&inner);
    handle_pressed(&inner).await;
}

fn switch_to_previous_style(inner: &Arc<Inner>) {
    let mut prefs = inner.prefs.get();
    let packs = match inner.style_packs.list() {
        Ok(packs) => packs,
        Err(error) => {
            log::warn!("[coord] switch style hotkey failed to load style packs: {error}");
            return;
        }
    };
    let enabled: Vec<crate::types::StylePack> =
        packs.into_iter().filter(|pack| pack.enabled).collect();
    if enabled.len() <= 1 {
        log::info!("[coord] switch style hotkey ignored: enabled style count <= 1");
        return;
    }
    let current_index = enabled
        .iter()
        .position(|pack| pack.id == prefs.active_style_pack_id)
        .unwrap_or(0);
    let next_index = if current_index == 0 {
        enabled.len() - 1
    } else {
        current_index - 1
    };
    prefs.active_style_pack_id = enabled[next_index].id.clone();
    sync_style_pack_preferences(&mut prefs, &enabled);
    if let Err(e) = inner.prefs.set(prefs.clone()) {
        log::warn!("[coord] switch style hotkey 保存失败: {e}");
    } else {
        log::info!(
            "[coord] switch style hotkey changed active style pack to {}",
            prefs.active_style_pack_id
        );
        if let Some(app) = inner.app.lock().clone() {
            let _ = app.emit("prefs:changed", &prefs);
            let _ = app.emit_to("main", "prefs:changed", &prefs);
            let app_for_main = app.clone();
            let _ = app.run_on_main_thread(move || {
                if let Err(err) = crate::refresh_tray_microphone_menu(&app_for_main) {
                    log::warn!("[tray] refresh style menu after switch style hotkey failed: {err}");
                }
            });
        }
    }
}

fn take_combo_hotkey_on_main_thread(inner: &Arc<Inner>) {
    let app = inner.app.lock().clone();
    if let Some(app) = app {
        let inner = Arc::clone(inner);
        let _ = app.run_on_main_thread(move || {
            inner.combo_hotkey.lock().take();
        });
    } else {
        inner.combo_hotkey.lock().take();
    }
}

fn take_translation_hotkey_on_main_thread(inner: &Arc<Inner>) {
    let app = inner.app.lock().clone();
    if let Some(app) = app {
        let inner = Arc::clone(inner);
        let _ = app.run_on_main_thread(move || {
            inner.translation_hotkey.lock().take();
        });
    } else {
        inner.translation_hotkey.lock().take();
    }
}

fn take_action_hotkey_on_main_thread(inner: &Arc<Inner>, kind: ActionHotkeyKind) {
    let app = inner.app.lock().clone();
    if let Some(app) = app {
        let inner = Arc::clone(inner);
        let _ = app.run_on_main_thread(move || {
            action_hotkey_slot(&inner, kind).lock().take();
        });
    } else {
        action_hotkey_slot(inner, kind).lock().take();
    }
}

fn action_hotkey_slot(
    inner: &Arc<Inner>,
    kind: ActionHotkeyKind,
) -> &Mutex<Option<ComboHotkeyMonitor>> {
    match kind {
        ActionHotkeyKind::SwitchStyle => &inner.switch_style_hotkey,
        ActionHotkeyKind::OpenApp => &inner.open_app_hotkey,
        ActionHotkeyKind::DeviceKey { key, gesture } => {
            &inner.device_key_hotkeys[device_key_hotkey_index(key, gesture)]
        }
    }
}

fn device_key_hotkey_index(key: DeviceCustomKeyId, gesture: DeviceCustomKeyGesture) -> usize {
    if key == DeviceCustomKeyId::Knob {
        return 12;
    }
    let gesture_offset = match gesture {
        DeviceCustomKeyGesture::SingleClick => 0,
        DeviceCustomKeyGesture::DoubleClick => 4,
        DeviceCustomKeyGesture::LongPress => 8,
    };
    let key_offset = match key {
        DeviceCustomKeyId::Key1 => 0,
        DeviceCustomKeyId::Key2 => 1,
        DeviceCustomKeyId::Key3 => 2,
        DeviceCustomKeyId::Key4 => 3,
        DeviceCustomKeyId::Knob => 0,
    };
    gesture_offset + key_offset
}

fn action_hotkey_binding(
    inner: &Arc<Inner>,
    kind: ActionHotkeyKind,
) -> crate::types::ShortcutBinding {
    let prefs = inner.prefs.get();
    match kind {
        ActionHotkeyKind::SwitchStyle => prefs.switch_style_hotkey,
        ActionHotkeyKind::OpenApp => prefs.open_app_hotkey,
        ActionHotkeyKind::DeviceKey { key, gesture } => crate::types::ShortcutBinding {
            primary: key.fallback_primary_for(gesture).into(),
            modifiers: if key == DeviceCustomKeyId::Knob {
                vec!["shift".into()]
            } else {
                Vec::new()
            },
        },
    }
}

fn is_modifier_only_shortcut(binding: &crate::types::ShortcutBinding) -> bool {
    binding.modifiers.is_empty()
        && (binding.primary.eq_ignore_ascii_case("shift")
            || crate::shortcut_binding::legacy_modifier_trigger(binding).is_some())
}

fn is_unconfigured_shortcut(binding: &crate::types::ShortcutBinding) -> bool {
    binding.primary.trim().is_empty()
}

fn action_hotkey_bridge_thread_name(kind: ActionHotkeyKind) -> &'static str {
    match kind {
        ActionHotkeyKind::SwitchStyle => "listener-type-switch-style-hotkey-bridge",
        ActionHotkeyKind::OpenApp => "listener-type-open-app-hotkey-bridge",
        ActionHotkeyKind::DeviceKey { .. } => "listener-type-device-key-hotkey-bridge",
    }
}

fn is_builtin_translation_shift(binding: &crate::types::ShortcutBinding) -> bool {
    binding.modifiers.is_empty() && binding.primary.eq_ignore_ascii_case("shift")
}

fn modifier_shortcut_triggers(
    inner: &Arc<Inner>,
) -> (
    Option<crate::types::HotkeyTrigger>,
    Option<crate::types::HotkeyTrigger>,
) {
    let prefs = inner.prefs.get();
    let qa_trigger = prefs
        .qa_hotkey
        .as_ref()
        .and_then(crate::shortcut_binding::legacy_modifier_trigger);
    let translation_trigger = if is_builtin_translation_shift(&prefs.translation_hotkey) {
        None
    } else {
        crate::shortcut_binding::legacy_modifier_trigger(&prefs.translation_hotkey)
    };
    (qa_trigger, translation_trigger)
}
