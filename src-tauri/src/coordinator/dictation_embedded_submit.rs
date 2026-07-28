// Embedded audio / BLE submit entrypoints.
// Included into `coordinator::dictation` via `include!`.

pub(super) async fn submit_embedded_audio_notifications(
    inner: &Arc<Inner>,
    notifications: Vec<Vec<u8>>,
) -> Result<crate::embedded_audio::EmbeddedAudioSubmissionResult, String> {
    let collector =
        crate::embedded_audio::collect_notifications(notifications.iter().map(Vec::as_slice))
            .map_err(|err| format!("嵌入式音频包解析失败: {err}"))?;
    let stats = collector.stats();
    if !stats.terminal_received {
        return Err("嵌入式音频会话尚未收到结束包".to_string());
    }
    if stats.end_reason != Some(crate::embedded_audio::SessionEndReason::Stop) {
        return Err(format!("嵌入式音频会话未正常结束: {:?}", stats.end_reason));
    }

    let pcm = collector.reconstructed_asr_boundary_pcm();
    if pcm.is_empty() {
        return Err("嵌入式音频会话没有可识别的 PCM 数据".to_string());
    }
    if pcm.len() % 2 != 0 {
        return Err("嵌入式音频 PCM 长度不是 16-bit 对齐".to_string());
    }

    let reconstructed_pcm_bytes = stats.reconstructed_pcm_bytes;
    if stats.post_stop_packet_count > 0 {
        log::info!(
            "[coord] embedded audio batch excluded post-stop tail from ASR (tail_packets={}, tail_pcm_bytes={}, asr_pcm_bytes={}, reconstructed_pcm_bytes={})",
            stats.post_stop_packet_count,
            stats.post_stop_pcm_bytes,
            stats.asr_boundary_pcm_bytes,
            stats.reconstructed_pcm_bytes
        );
    }
    submit_embedded_pcm_for_dictation_with_stats(inner, &pcm, Some(stats.clone())).await?;
    let transcript = take_latest_embedded_audio_final_result(inner);
    Ok(crate::embedded_audio::EmbeddedAudioSubmissionResult {
        stats,
        reconstructed_pcm_bytes,
        transcript,
    })
}

pub(super) async fn submit_embedded_audio_streaming_notifications(
    inner: &Arc<Inner>,
    notifications: Vec<Vec<u8>>,
) -> Result<crate::embedded_audio::EmbeddedAudioSubmissionResult, String> {
    let mut streaming = EmbeddedStreamingDictation::default();
    for notification in notifications {
        match streaming.handle_notification(inner, &notification).await {
            Ok(true) => break,
            Ok(false) => {}
            Err(err) => {
                streaming.abort_active_session(inner, &err);
                return Err(err);
            }
        }
    }
    if !streaming.terminal_received {
        streaming.abort_active_session(inner, "嵌入式音频流式会话尚未收到结束包");
    }
    streaming.into_submission_result()
}

pub(super) async fn submit_embedded_audio_file(
    inner: &Arc<Inner>,
    path: std::path::PathBuf,
    format: Option<crate::embedded_audio::EmbeddedAudioInputFormat>,
) -> Result<crate::embedded_audio::EmbeddedAudioSubmissionResult, String> {
    let pcm = crate::embedded_audio::read_input_pcm(&path, format)
        .map_err(|err| format!("读取嵌入式音频文件失败 ({}): {err}", path.display()))?;
    let notifications = crate::embedded_audio::build_session_replay_notifications(
        crate::embedded_audio::ReplayConfig {
            session_id: embedded_audio_file_session_id(),
            payload_pcm_bytes: crate::embedded_audio::DEFAULT_REPLAY_PAYLOAD_PCM_BYTES,
        },
        &pcm,
    )
    .map_err(|err| format!("构造嵌入式音频回放包失败: {err}"))?;
    submit_embedded_audio_notifications(inner, notifications).await
}

pub(super) async fn submit_embedded_audio_streaming_file(
    inner: &Arc<Inner>,
    path: std::path::PathBuf,
    format: Option<crate::embedded_audio::EmbeddedAudioInputFormat>,
) -> Result<crate::embedded_audio::EmbeddedAudioSubmissionResult, String> {
    let pcm = crate::embedded_audio::read_input_pcm(&path, format)
        .map_err(|err| format!("读取嵌入式音频文件失败 ({}): {err}", path.display()))?;
    let notifications = crate::embedded_audio::build_session_replay_notifications(
        crate::embedded_audio::ReplayConfig {
            session_id: embedded_audio_file_session_id(),
            payload_pcm_bytes: crate::embedded_audio::DEFAULT_REPLAY_PAYLOAD_PCM_BYTES,
        },
        &pcm,
    )
    .map_err(|err| format!("构造嵌入式音频流式回放包失败: {err}"))?;
    let mut streaming = EmbeddedStreamingDictation::default();
    for notification in notifications {
        let packet_duration = crate::embedded_audio::parse_packet(&notification)
            .ok()
            .filter(|packet| {
                packet.header.packet_type == crate::embedded_audio::PacketType::AudioData
            })
            .map(|packet| {
                Duration::from_secs_f64(
                    packet.header.packet_pcm_bytes as f64
                        / crate::embedded_audio::PCM_BYTES_PER_SECOND as f64,
                )
            });
        match streaming.handle_notification(inner, &notification).await {
            Ok(true) => break,
            Ok(false) => {}
            Err(err) => {
                streaming.abort_active_session(inner, &err);
                return Err(err);
            }
        }
        if let Some(duration) = packet_duration {
            tokio::time::sleep(duration).await;
        }
    }
    if !streaming.terminal_received {
        streaming.abort_active_session(inner, "嵌入式音频流式文件回放尚未收到结束包");
    }
    streaming.into_submission_result()
}

pub(super) async fn submit_embedded_audio_ble_once(
    inner: &Arc<Inner>,
    timeout_ms: Option<u64>,
) -> Result<crate::embedded_audio::EmbeddedAudioSubmissionResult, String> {
    let timeout = std::time::Duration::from_millis(timeout_ms.unwrap_or(120_000).max(1_000));
    if embedded_ble_stats_only_enabled() {
        log::info!(
            "[embedded-ble] headless stats-only one-shot enabled by {EMBEDDED_BLE_STATS_ONLY_ENV}"
        );
        return submit_embedded_audio_ble_stats_only(timeout).await;
    }

    let notifications = tauri::async_runtime::spawn_blocking(move || {
        crate::embedded_ble::capture_notifications_once(timeout)
    })
    .await
    .map_err(|err| format!("嵌入式 BLE 抓音任务失败: {err}"))??;
    submit_embedded_audio_notifications(inner, notifications).await
}

pub(super) async fn submit_embedded_audio_ble_stream(
    inner: &Arc<Inner>,
    timeout_ms: Option<u64>,
) -> Result<crate::embedded_audio::EmbeddedAudioSubmissionResult, String> {
    submit_embedded_audio_ble_stream_impl(
        inner,
        timeout_ms,
        true,
        Arc::new(AtomicBool::new(false)),
        None,
    )
    .await
}

pub(super) async fn submit_embedded_audio_ble_stream_background(
    inner: &Arc<Inner>,
    cancel_capture: Arc<AtomicBool>,
    leave_notify_cccd_enabled_on_cancel: Arc<AtomicBool>,
) -> Result<crate::embedded_audio::EmbeddedAudioSubmissionResult, String> {
    submit_embedded_audio_ble_stream_impl(
        inner,
        None,
        false,
        cancel_capture,
        Some(leave_notify_cccd_enabled_on_cancel),
    )
    .await
}

fn embedded_ble_stream_idle_timeout(
    timeout: Duration,
    emit_idle_capture_errors: bool,
) -> Option<Duration> {
    emit_idle_capture_errors.then_some(timeout)
}

enum EmbeddedBleStreamSignal {
    Ready,
    Notification(Vec<u8>),
}

fn embedded_ble_stats_only_enabled() -> bool {
    std::env::var(EMBEDDED_BLE_STATS_ONLY_ENV)
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

fn configured_embedded_ble_control_signal_path(env_name: &str) -> Option<String> {
    std::env::var(env_name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn wait_for_embedded_ble_control_signal(path: &str, timeout: Duration, label: &str) -> bool {
    let started = Instant::now();
    while started.elapsed() < timeout {
        if std::path::Path::new(path).exists() {
            if let Err(err) = fs::remove_file(path) {
                log::warn!("[embedded-ble] {label} signal remove failed ({path}): {err}");
            }
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    false
}

fn start_embedded_ble_control_signal_worker_if_configured() {
    let start_signal =
        configured_embedded_ble_control_signal_path(EMBEDDED_BLE_CONTROL_START_SIGNAL_ENV);
    let stop_signal =
        configured_embedded_ble_control_signal_path(EMBEDDED_BLE_CONTROL_STOP_SIGNAL_ENV);
    if start_signal.is_none() && stop_signal.is_none() {
        return;
    }

    let _ = std::thread::Builder::new()
        .name("listener-type-embedded-ble-control-signals".into())
        .spawn(move || {
            log::info!(
                "[embedded-ble] headless control signal worker started start_signal={:?} stop_signal={:?}",
                start_signal,
                stop_signal
            );
            if let Some(path) = start_signal.as_deref() {
                if wait_for_embedded_ble_control_signal(path, Duration::from_secs(60), "start") {
                    match crate::embedded_ble::send_recording_control_toggle(
                        EMBEDDED_BLE_RECORDING_CONTROL_WRITE_TIMEOUT,
                    ) {
                        Ok(()) => log::info!(
                            "[embedded-ble] headless control start signal sent VREC:TOGGLE via Type"
                        ),
                        Err(err) => log::warn!(
                            "[embedded-ble] headless control start signal failed: {err}"
                        ),
                    }
                } else {
                    log::warn!(
                        "[embedded-ble] headless control start signal timed out waiting for {path}"
                    );
                    return;
                }
            }
            if let Some(path) = stop_signal.as_deref() {
                if wait_for_embedded_ble_control_signal(path, Duration::from_secs(300), "stop") {
                    match crate::embedded_ble::send_recording_control_stop(
                        EMBEDDED_BLE_RECORDING_CONTROL_WRITE_TIMEOUT,
                    ) {
                        Ok(()) => log::info!(
                            "[embedded-ble] headless control stop signal sent VREC:STOP via Type"
                        ),
                        Err(err) => log::warn!(
                            "[embedded-ble] headless control stop signal failed: {err}"
                        ),
                    }
                } else {
                    log::warn!(
                        "[embedded-ble] headless control stop signal timed out waiting for {path}"
                    );
                }
            }
        });
}

async fn submit_embedded_audio_ble_stats_only(
    timeout: Duration,
) -> Result<crate::embedded_audio::EmbeddedAudioSubmissionResult, String> {
    // Headless machine path: optional VREC start/stop signal files drive the
    // device while we only collect PCM stats (no ASR vault / dictation).
    start_embedded_ble_control_signal_worker_if_configured();
    let notifications = tauri::async_runtime::spawn_blocking(move || {
        crate::embedded_ble::capture_notifications_once(timeout)
    })
    .await
    .map_err(|err| format!("嵌入式 BLE stats-only 抓音任务失败: {err}"))??;
    let collector =
        crate::embedded_audio::collect_notifications(notifications.iter().map(Vec::as_slice))
            .map_err(|err| format!("嵌入式 BLE stats-only 包解析失败: {err}"))?;
    let stats = collector.stats();
    if !stats.terminal_received {
        return Err("嵌入式 BLE stats-only 会话尚未收到结束包".to_string());
    }
    if stats.end_reason != Some(crate::embedded_audio::SessionEndReason::Stop) {
        return Err(format!(
            "嵌入式 BLE stats-only 会话未正常结束: {:?}",
            stats.end_reason
        ));
    }
    if stats.reconstructed_pcm_bytes == 0 {
        return Err("嵌入式 BLE stats-only 会话没有可识别的 PCM 数据".to_string());
    }
    log::info!(
        "[embedded-ble] stats-only capture done: pcm_bytes={} missing_packets={} received_packets={}",
        stats.reconstructed_pcm_bytes,
        stats.missing_packet_count,
        stats.received_packet_count
    );
    Ok(crate::embedded_audio::EmbeddedAudioSubmissionResult {
        reconstructed_pcm_bytes: stats.reconstructed_pcm_bytes,
        stats,
        transcript: None,
    })
}

async fn submit_embedded_audio_ble_stream_impl(
    inner: &Arc<Inner>,
    timeout_ms: Option<u64>,
    emit_idle_capture_errors: bool,
    cancel_capture: Arc<AtomicBool>,
    leave_notify_cccd_enabled_on_cancel: Option<Arc<AtomicBool>>,
) -> Result<crate::embedded_audio::EmbeddedAudioSubmissionResult, String> {
    let timeout = std::time::Duration::from_millis(timeout_ms.unwrap_or(120_000).max(1_000));
    if emit_idle_capture_errors && embedded_ble_stats_only_enabled() {
        log::info!(
            "[embedded-ble] headless stats-only stream enabled by {EMBEDDED_BLE_STATS_ONLY_ENV}"
        );
        return submit_embedded_audio_ble_stats_only(timeout).await;
    }

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<EmbeddedBleStreamSignal>();
    // Continuous background notify must not share its capture-cancel flag with
    // dictation cancel. Capsule/Esc cancel used to set that flag, close CCCD,
    // send TYPE:BYE, and force a full GATT reopen — the main stability thrash.
    // One-shot foreground still cancels capture via the same flag.
    let session_abort = if emit_idle_capture_errors {
        register_embedded_ble_cancel_flag(inner, &cancel_capture);
        None
    } else {
        let session_abort = Arc::new(AtomicBool::new(false));
        register_embedded_ble_cancel_flag(inner, &session_abort);
        Some(session_abort)
    };
    let cancel_capture_for_task = Arc::clone(&cancel_capture);
    let leave_notify_cccd_enabled_on_cancel_for_task =
        leave_notify_cccd_enabled_on_cancel.map(|handoff| Arc::clone(&handoff));
    let ready_inner = (!emit_idle_capture_errors).then(|| Arc::clone(inner));
    let ready_cancel = Arc::clone(&cancel_capture);
    let capture_task = tauri::async_runtime::spawn_blocking(move || {
        let ready_tx = tx.clone();
        let mut on_ready = || {
            if let Some(inner) = ready_inner.as_ref() {
                mark_embedded_ble_listener_ready(inner, &ready_cancel);
            } else {
                ready_tx
                    .send(EmbeddedBleStreamSignal::Ready)
                    .map_err(|_| "嵌入式音频流式处理已结束".to_string())?;
            }
            Ok(())
        };
        let capture_result = if emit_idle_capture_errors {
            crate::embedded_ble::capture_notification_events_until_cancelled(
                embedded_ble_stream_idle_timeout(timeout, emit_idle_capture_errors),
                cancel_capture_for_task,
                &mut on_ready,
                &mut |event| {
                    tx.send(EmbeddedBleStreamSignal::Notification(event.notification))
                        .map_err(|_| "嵌入式音频流式处理已结束".to_string())
                },
            )
        } else {
            crate::embedded_ble::capture_notification_events_continuous_with_connection_handoff_until_cancelled(
                embedded_ble_stream_idle_timeout(timeout, emit_idle_capture_errors),
                cancel_capture_for_task,
                leave_notify_cccd_enabled_on_cancel_for_task
                    .expect("background Listener capture always has a cancellation handoff flag"),
                &mut on_ready,
                &mut |event| {
                    tx.send(EmbeddedBleStreamSignal::Notification(event.notification))
                        .map_err(|_| "嵌入式音频流式处理已结束".to_string())
                },
            )
        };
        capture_result
    });

    let mut streaming = if emit_idle_capture_errors {
        EmbeddedStreamingDictation::default()
    } else {
        EmbeddedStreamingDictation::background_listener()
    };
    let mut ready_capsule_shown = false;
    let mut control_signal_worker_started = false;
    loop {
        // Soft session abort (continuous only): wake without waiting for more PCM.
        if let Some(session_abort) = session_abort.as_ref() {
            if session_abort.swap(false, Ordering::SeqCst) {
                let _ = streaming.discard_active_session_after_user_cancel(inner);
                record_embedded_ble_session_actor_command(
                    inner,
                    EmbeddedBleSessionActorCommand::ActorRestart,
                    None,
                    "background listener kept notify open after user session cancel",
                );
            }
        }
        // Continuous: finalize STOP with missing packets so capture can keep
        // TYPE:READY instead of waiting forever for a perfect drain.
        if !emit_idle_capture_errors {
            match streaming.force_finish_pending_stop_if_due(inner).await {
                Ok(true) => {
                    streaming.reset_for_next_session();
                    record_embedded_ble_session_actor_command(
                        inner,
                        EmbeddedBleSessionActorCommand::ActorRestart,
                        None,
                        "background listener ready after forced stop finish without reopening notify",
                    );
                    continue;
                }
                Ok(false) => {}
                Err(err) => {
                    log::warn!(
                        "[embedded-ble] continuous forced stop finish error while keeping notify: {err}"
                    );
                    streaming.discard_active_session_after_stream_error(inner, &err);
                    continue;
                }
            }
        }

        let maybe_signal = if session_abort.is_some() || !emit_idle_capture_errors {
            tokio::select! {
                signal = rx.recv() => signal,
                _ = tokio::time::sleep(Duration::from_millis(40)) => {
                    // Poll soft-cancel / forced stop finish promptly when quiet.
                    continue;
                }
            }
        } else {
            rx.recv().await
        };

        let Some(signal) = maybe_signal else {
            break;
        };
        let notification = match signal {
            EmbeddedBleStreamSignal::Ready => {
                if emit_idle_capture_errors && !control_signal_worker_started {
                    control_signal_worker_started = true;
                    start_embedded_ble_control_signal_worker_if_configured();
                }
                if emit_idle_capture_errors && !ready_capsule_shown {
                    ready_capsule_shown = true;
                    emit_capsule(
                        inner,
                        CapsuleState::Recording,
                        0.0,
                        0,
                        Some(EMBEDDED_BLE_READY_CAPSULE_MESSAGE.to_string()),
                        None,
                    );
                }
                continue;
            }
            EmbeddedBleStreamSignal::Notification(notification) => notification,
        };
        // Re-check soft cancel before applying a packet that may race the cancel.
        if let Some(session_abort) = session_abort.as_ref() {
            if session_abort.swap(false, Ordering::SeqCst) {
                let _ = streaming.discard_active_session_after_user_cancel(inner);
            }
        }
        match streaming.handle_notification(inner, &notification).await {
            Ok(true) => {
                if emit_idle_capture_errors {
                    cancel_capture.store(true, Ordering::SeqCst);
                    break;
                }
                match streaming.submission_result() {
                    Ok(result) => {
                        log::info!(
                            "[embedded-ble] background session completed while keeping notify open pcm_bytes={} missing_packets={}",
                            result.reconstructed_pcm_bytes,
                            result.stats.missing_packet_count
                        );
                    }
                    Err(err) => {
                        // Continuous path: logical post-stop failure must not tear
                        // down TYPE:READY; reset and keep listening.
                        log::warn!(
                            "[embedded-ble] background session submission incomplete while keeping notify open: {err}"
                        );
                        streaming.discard_active_session_after_stream_error(inner, &err);
                        streaming.reset_for_next_session();
                        continue;
                    }
                }
                streaming.reset_for_next_session();
                record_embedded_ble_session_actor_command(
                    inner,
                    EmbeddedBleSessionActorCommand::ActorRestart,
                    None,
                    "background listener ready for next BLE session without reopening notify",
                );
            }
            Ok(false) => {}
            Err(err) => {
                if !emit_idle_capture_errors {
                    streaming.discard_active_session_after_stream_error(inner, &err);
                    record_embedded_ble_session_actor_command(
                        inner,
                        EmbeddedBleSessionActorCommand::ActorRestart,
                        None,
                        format!("background listener kept notify open after stream error: {err}"),
                    );
                    continue;
                }
                streaming.abort_active_session(inner, &err);
                cancel_capture.store(true, Ordering::SeqCst);
                clear_embedded_ble_cancel_flag(inner, &cancel_capture);
                return Err(err);
            }
        }
    }
    let capture_cancel_requested = cancel_capture.load(Ordering::SeqCst);
    cancel_capture.store(true, Ordering::SeqCst);

    let capture_result = capture_task
        .await
        .map_err(|err| format!("嵌入式 BLE 流式抓音任务失败: {err}"))
        .and_then(|result| result);
    if let Some(session_abort) = session_abort.as_ref() {
        clear_embedded_ble_cancel_flag(inner, session_abort);
    } else {
        clear_embedded_ble_cancel_flag(inner, &cancel_capture);
    }
    if let Err(err) = &capture_result {
        if !emit_idle_capture_errors
            && crate::embedded_ble::is_background_listener_deferred_for_ota_error(err)
        {
            return Err(err.clone());
        }
    }
    let cancelled_by_caller = !streaming.terminal_received
        && (streaming.session.is_some() || streaming.embedded_session_id.is_some())
        && inner.state.lock().cancelled;
    if capture_result.is_ok() && cancelled_by_caller {
        // Continuous path should soft-cancel before capture ends; if capture still
        // ends with leftover session state (refresh during cancel), treat as cancel.
        log::info!("[embedded-ble] streaming capture stopped after dictation cancel");
        return Ok(streaming.into_cancelled_submission_result());
    }
    if capture_result.is_ok()
        && !emit_idle_capture_errors
        && capture_cancel_requested
        && !streaming.terminal_received
        && streaming.session.is_none()
        && streaming.embedded_session_id.is_none()
    {
        return Err("嵌入式 BLE 后台监听已取消，尚未开始录音会话".to_string());
    }
    if let Err(err) = capture_result {
        if !streaming.terminal_received {
            record_embedded_ble_recovery_failure(inner, &err);
            let guidance = embedded_ble_wake_guidance_for_error(&err);
            let message = if emit_idle_capture_errors {
                format!("嵌入式 BLE 流式抓音中断: {guidance}")
            } else {
                format!("嵌入式 BLE 流式抓音中断: {guidance}; cause={err}")
            };
            if emit_idle_capture_errors || streaming.session.is_some() {
                streaming.abort_active_session(inner, &message);
            }
            return Err(message);
        }
    }
    if !streaming.terminal_received {
        if streaming.collector.inner().has_stopped_with_audio() {
            streaming.finish_pending_stop_after_capture(inner).await?;
        } else {
            if emit_idle_capture_errors || streaming.session.is_some() {
                streaming.abort_active_session(inner, "嵌入式 BLE 流式会话尚未收到结束包");
            }
        }
    }
    streaming.into_submission_result()
}
