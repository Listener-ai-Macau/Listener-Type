// BLE capture event loop + active-control helpers (Windows).
// Included into `windows_ble` via `include!`.

const BLE_INGRESS_DIAGNOSTIC_DIR_ENV: &str = "LISTENER_WAKE_DIAGNOSTIC_DIR";
const BLE_INGRESS_DIAGNOSTIC_CHANNEL_CAPACITY: usize = 512;
const BLE_INGRESS_DIAGNOSTIC_MAX_PCM_BYTES: usize = 12 * 16_000 * 2;

enum BleIngressDiagnosticMessage {
    Received {
        received_unix_ms: u64,
        notification: Vec<u8>,
    },
    Collector {
        capture_generation: u64,
        handled_unix_ms: u64,
        admission_fact: Option<crate::embedded_audio::SessionAdmissionFact>,
        notification: Vec<u8>,
    },
}

#[derive(Clone)]
struct BleIngressDiagnosticSink {
    tx: mpsc::SyncSender<BleIngressDiagnosticMessage>,
    dropped_count: Arc<AtomicUsize>,
}

impl BleIngressDiagnosticSink {
    fn for_capture(capture_id: u64) -> Option<Self> {
        let directory = std::env::var(BLE_INGRESS_DIAGNOSTIC_DIR_ENV)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)?;
        if let Err(err) = fs::create_dir_all(&directory) {
            log::warn!(
                "[embedded-ble] ingress diagnostic directory unavailable capture_id={capture_id}: {err}"
            );
            return None;
        }
        let timestamp_ms = ble_ingress_diagnostic_unix_ms();
        let stem = format!(
            "ble-ingress-capture-{capture_id}-{}-{timestamp_ms}",
            std::process::id()
        );
        let events_path = directory.join(format!("{stem}-events.jsonl"));
        let raw_pcm_path = directory.join(format!("{stem}-raw.pcm"));
        let collector_pcm_path = directory.join(format!("{stem}-collector.pcm"));
        let (tx, rx) = mpsc::sync_channel(BLE_INGRESS_DIAGNOSTIC_CHANNEL_CAPACITY);
        let dropped_count = Arc::new(AtomicUsize::new(0));
        let dropped_count_for_worker = Arc::clone(&dropped_count);
        let spawn = std::thread::Builder::new()
            .name(format!("listener-type-ble-ingress-{capture_id}"))
            .spawn(move || {
                run_ble_ingress_diagnostic_worker(
                    capture_id,
                    events_path,
                    raw_pcm_path,
                    collector_pcm_path,
                    rx,
                    dropped_count_for_worker,
                );
            });
        if let Err(err) = spawn {
            log::warn!(
                "[embedded-ble] ingress diagnostic worker unavailable capture_id={capture_id}: {err}"
            );
            return None;
        }
        Some(Self { tx, dropped_count })
    }

    fn record_received(&self, notification: &[u8]) {
        self.try_send(BleIngressDiagnosticMessage::Received {
            received_unix_ms: ble_ingress_diagnostic_unix_ms(),
            notification: notification.to_vec(),
        });
    }

    fn record_collector(
        &self,
        capture_generation: u64,
        admission_fact: Option<crate::embedded_audio::SessionAdmissionFact>,
        notification: &[u8],
    ) {
        self.try_send(BleIngressDiagnosticMessage::Collector {
            capture_generation,
            handled_unix_ms: ble_ingress_diagnostic_unix_ms(),
            admission_fact,
            notification: notification.to_vec(),
        });
    }

    fn try_send(&self, message: BleIngressDiagnosticMessage) {
        if self.tx.try_send(message).is_err() {
            if self.dropped_count.fetch_add(1, Ordering::Relaxed) == 0 {
                log::warn!(
                    "[embedded-ble] ingress diagnostic queue dropped at least one record; audio flow is unaffected"
                );
            }
        }
    }
}

fn ble_ingress_diagnostic_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

fn ble_ingress_admission_fact_json(
    fact: Option<&crate::embedded_audio::SessionAdmissionFact>,
) -> serde_json::Value {
    let Some(fact) = fact else {
        return serde_json::Value::Null;
    };
    serde_json::json!({
        "collectorInstanceId": fact.collector_instance_id,
        "resetEpoch": fact.reset_epoch,
        "notificationId": fact.notification_id,
        "admissionId": fact.admission_id,
        "physicalSessionId": fact.physical_session_id,
        "packetSequence": fact.packet_sequence,
        "disposition": serde_json::to_value(fact.disposition).unwrap_or(serde_json::Value::Null),
        "supersedesAdmissionId": fact.supersedes_admission_id,
        "predecessorUnknown": fact.predecessor_unknown,
        "afterStopBoundary": fact.after_stop_boundary,
        "wirePayloadBytes": fact.wire_payload_bytes,
        "declaredPcmBytes": fact.declared_pcm_bytes,
        "expandedPcmBytes": fact.expanded_pcm_bytes,
        "metadataIncomplete": fact.metadata_incomplete,
    })
}

fn write_ble_ingress_json_line(
    writer: &mut std::io::BufWriter<std::fs::File>,
    value: serde_json::Value,
) -> bool {
    if serde_json::to_writer(&mut *writer, &value).is_err()
        || writer.write_all(b"\n").is_err()
        || writer.flush().is_err()
    {
        return false;
    }
    true
}

fn insert_ble_ingress_pcm_bounded(
    packets: &mut std::collections::BTreeMap<(u32, u16), Vec<u8>>,
    stored_bytes: &mut usize,
    key: (u32, u16),
    pcm: Vec<u8>,
) -> bool {
    let old_bytes = packets.get(&key).map(Vec::len).unwrap_or(0);
    let Some(next_bytes) = stored_bytes
        .checked_sub(old_bytes)
        .and_then(|value| value.checked_add(pcm.len()))
    else {
        return false;
    };
    if next_bytes > BLE_INGRESS_DIAGNOSTIC_MAX_PCM_BYTES {
        return false;
    }
    packets.insert(key, pcm);
    *stored_bytes = next_bytes;
    true
}

fn write_ble_ingress_pcm_file(
    path: &std::path::Path,
    packets: &std::collections::BTreeMap<(u32, u16), Vec<u8>>,
) -> (usize, String) {
    let mut pcm = Vec::new();
    let mut session_id = None;
    for ((packet_session_id, _packet_sequence), packet_pcm) in packets {
        session_id.get_or_insert(*packet_session_id);
        if pcm.len().saturating_add(packet_pcm.len()) > BLE_INGRESS_DIAGNOSTIC_MAX_PCM_BYTES {
            break;
        }
        pcm.extend_from_slice(packet_pcm);
    }
    let bytes = pcm.len();
    let hash = crate::firmware_ota::sha256_hex(&pcm);
    if let Err(err) = fs::write(path, &pcm) {
        log::warn!(
            "[embedded-ble] ingress diagnostic PCM write failed path={} session_id={session_id:?}: {err}",
            path.display()
        );
    }
    (bytes, hash)
}

fn run_ble_ingress_diagnostic_worker(
    capture_id: u64,
    events_path: PathBuf,
    raw_pcm_path: PathBuf,
    collector_pcm_path: PathBuf,
    rx: mpsc::Receiver<BleIngressDiagnosticMessage>,
    dropped_count: Arc<AtomicUsize>,
) {
    let Ok(events_file) = fs::File::create(&events_path) else {
        log::warn!(
            "[embedded-ble] ingress diagnostic events file unavailable capture_id={} path={}",
            capture_id,
            events_path.display()
        );
        return;
    };
    let mut events = std::io::BufWriter::new(events_file);
    let mut raw_packets = std::collections::BTreeMap::new();
    let mut raw_stored_bytes = 0usize;
    let mut collector_packets = std::collections::BTreeMap::new();
    let mut collector_stored_bytes = 0usize;
    while let Ok(message) = rx.recv() {
        match message {
            BleIngressDiagnosticMessage::Received {
                received_unix_ms,
                notification,
            } => {
                let raw_hash = crate::firmware_ota::sha256_hex(&notification);
                // The BLE callback sees the wire packet, which may carry the
                // lossless Rice flag. Decode it through the same production
                // normalizer used immediately before collector admission so
                // the diagnostic compares PCM with the collector's PCM. Keep
                // the original notification hash/size above as the wire fact.
                let normalized_notification =
                    crate::audio_transport_codec::normalize_listener_audio_notification(
                        &notification,
                    )
                    .ok();
                let parsed = normalized_notification
                    .as_deref()
                    .and_then(|normalized| crate::embedded_audio::parse_packet(normalized).ok());
                let mut session_id = None;
                let mut packet_sequence = None;
                let mut packet_type = None;
                let mut pcm_hash = None;
                let mut pcm_bytes = 0usize;
                if let Some(packet) = parsed {
                    session_id = Some(packet.header.session_id);
                    packet_sequence = Some(packet.header.packet_sequence);
                    packet_type = Some(format!("{:?}", packet.header.packet_type));
                    if packet.header.packet_type == crate::embedded_audio::PacketType::AudioData {
                        let pcm = packet.expanded_payload_pcm();
                        pcm_bytes = pcm.len();
                        pcm_hash = Some(crate::firmware_ota::sha256_hex(&pcm));
                        let _ = insert_ble_ingress_pcm_bounded(
                            &mut raw_packets,
                            &mut raw_stored_bytes,
                            (packet.header.session_id, packet.header.packet_sequence),
                            pcm,
                        );
                    }
                }
                let _ = write_ble_ingress_json_line(
                    &mut events,
                    serde_json::json!({
                        "boundary": "ble_receive",
                        "captureGeneration": capture_id,
                        "receivedUnixMs": received_unix_ms,
                        "notificationBytes": notification.len(),
                        "notificationSha256": raw_hash,
                        "packetType": packet_type,
                        "sessionId": session_id,
                        "packetSequence": packet_sequence,
                        "pcmBytes": pcm_bytes,
                        "pcmSha256": pcm_hash,
                    }),
                );
            }
            BleIngressDiagnosticMessage::Collector {
                capture_generation,
                handled_unix_ms,
                admission_fact,
                notification,
            } => {
                let normalized_hash = crate::firmware_ota::sha256_hex(&notification);
                let parsed = crate::embedded_audio::parse_packet(&notification).ok();
                let (mut session_id, mut packet_sequence, mut packet_type, mut pcm) =
                    (None, None, None, Vec::new());
                if let Some(packet) = parsed {
                    session_id = Some(packet.header.session_id);
                    packet_sequence = Some(packet.header.packet_sequence);
                    packet_type = Some(format!("{:?}", packet.header.packet_type));
                    if packet.header.packet_type == crate::embedded_audio::PacketType::AudioData {
                        pcm = packet.expanded_payload_pcm();
                    }
                }
                if let (Some(session_id), Some(packet_sequence)) =
                    (session_id, packet_sequence)
                {
                    let accepted = admission_fact.as_ref().is_some_and(|fact| {
                        matches!(
                            fact.disposition,
                            crate::embedded_audio::SessionAdmissionDisposition::New
                                | crate::embedded_audio::SessionAdmissionDisposition::Replacement
                        )
                    });
                    if accepted && !pcm.is_empty() {
                        let _ = insert_ble_ingress_pcm_bounded(
                            &mut collector_packets,
                            &mut collector_stored_bytes,
                            (session_id, packet_sequence),
                            pcm.clone(),
                        );
                    }
                }
                let _ = write_ble_ingress_json_line(
                    &mut events,
                    serde_json::json!({
                        "boundary": "capture_collector",
                        "captureGeneration": capture_generation,
                        "handledUnixMs": handled_unix_ms,
                        "packetType": packet_type,
                        "sessionId": session_id,
                        "packetSequence": packet_sequence,
                        "normalizedNotificationBytes": notification.len(),
                        "normalizedNotificationSha256": normalized_hash,
                        "pcmBytes": pcm.len(),
                        "pcmSha256": if pcm.is_empty() { None } else { Some(crate::firmware_ota::sha256_hex(&pcm)) },
                        "admissionFact": ble_ingress_admission_fact_json(admission_fact.as_ref()),
                    }),
                );
            }
        }
    }
    let (raw_bytes, raw_hash) = write_ble_ingress_pcm_file(&raw_pcm_path, &raw_packets);
    let (collector_bytes, collector_hash) =
        write_ble_ingress_pcm_file(&collector_pcm_path, &collector_packets);
    let _ = write_ble_ingress_json_line(
        &mut events,
        serde_json::json!({
            "boundary": "capture_summary",
            "captureGeneration": capture_id,
            "rawPcmBytes": raw_bytes,
            "rawPcmSha256": raw_hash,
            "collectorPcmBytes": collector_bytes,
            "collectorPcmSha256": collector_hash,
            "diagnosticQueueDroppedCount": dropped_count.load(Ordering::Relaxed),
            "rawPcmPath": raw_pcm_path,
            "collectorPcmPath": collector_pcm_path,
        }),
    );
    log::info!(
        "[embedded-ble] ingress diagnostic complete capture_id={} raw_pcm_bytes={} collector_pcm_bytes={} dropped={}",
        capture_id,
        raw_bytes,
        collector_bytes,
        dropped_count.load(Ordering::Relaxed)
    );
}

fn send_audio_control_via_active_capture(
    bytes: &[u8],
    timeout: Duration,
    label: &str,
) -> Option<Result<(), String>> {
    let active = active_audio_control_sender()?;
    let (result_tx, result_rx) = mpsc::channel();
    let request = AudioControlRequest {
        bytes: bytes.to_vec(),
        label: label.to_string(),
        timeout,
        queued_at: Instant::now(),
        result_tx,
    };
    if active.tx.send(request).is_err() {
        clear_active_audio_control_sender(active.capture_id);
        return None;
    }
    let result = result_rx
        .recv_timeout(timeout + Duration::from_secs(1))
        .unwrap_or_else(|_| {
            Err(format!(
                "active Listener BLE audio control timed out after {} ms",
                timeout.as_millis()
            ))
        });
    if let Err(err) = &result {
        if is_transient_audio_control_write_error(err) {
            clear_active_audio_control_sender(active.capture_id);
        }
    }
    Some(result)
}

fn wait_post_ota_notify_target_connected(
    capture_id: u64,
    target: &OpenNotifyTarget,
) -> Result<(), String> {
    wait_post_ota_notify_target_connected_with_timeout(
        capture_id,
        target,
        POST_OTA_NOTIFY_LINK_READY_TIMEOUT,
    )
}

fn wait_post_ota_notify_target_connected_with_timeout(
    capture_id: u64,
    target: &OpenNotifyTarget,
    timeout: Duration,
) -> Result<(), String> {
    if !target.post_ota_preserved_cccd {
        return Ok(());
    }

    let started = Instant::now();
    let deadline = started + timeout;
    let mut connected_since = None;
    loop {
        if notify_capture_cancel_requested() {
            return Err(notify_capture_cancelled_error(
                "post-OTA notify link settle",
            ));
        }
        let device_connected = target.device.as_ref().is_none_or(|device| {
            device
                .ConnectionStatus()
                .is_ok_and(|status| status == BluetoothConnectionStatus::Connected)
        });
        let session_active = target.session.as_ref().is_none_or(|session| {
            session
                .SessionStatus()
                .is_ok_and(|status| status == GattSessionStatus::Active)
        });
        if device_connected && session_active {
            let stable_started = connected_since.get_or_insert_with(Instant::now);
            if stable_started.elapsed() >= POST_OTA_NOTIFY_LINK_STABLE_FOR {
                log::info!(
                    "[embedded-ble] capture #{capture_id}: post-OTA notify target stable before TYPE:READY elapsed_ms={} stable_ms={}",
                    started.elapsed().as_millis(),
                    stable_started.elapsed().as_millis()
                );
                return Ok(());
            }
        } else {
            connected_since = None;
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "post-OTA notify target did not become connected/active within {} ms",
                timeout.as_millis()
            ));
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn should_ignore_stale_disconnect_status(
    device_connected: bool,
    gatt_session_active: bool,
    active_capture: bool,
) -> bool {
    gatt_session_active && (device_connected || active_capture)
}

/// A transport notification is only an observation that the link delivered a
/// value. It must not consume the recovery deadline before normalize/collector
/// acceptance proves current-segment PCM progress.
fn active_capture_link_recovery_notification_pending(
    link_recovery_deadline: Option<Instant>,
) -> bool {
    link_recovery_deadline.is_some()
}

fn active_capture_link_recovery_expired(deadline: Option<Instant>, now: Instant) -> bool {
    deadline.is_some_and(|recovery_deadline| now >= recovery_deadline)
}

fn take_active_capture_link_recovery_after_valid_pcm(
    link_recovery_deadline: &mut Option<Instant>,
    notify_refresh_required_after_link_recovery: &mut bool,
) -> bool {
    if link_recovery_deadline.take().is_some() {
        *notify_refresh_required_after_link_recovery = true;
        true
    } else {
        false
    }
}

fn clear_active_capture_link_recovery_wait(
    link_recovery_deadline: &mut Option<Instant>,
    link_recovery_reason: &mut Option<String>,
) -> bool {
    if link_recovery_deadline.take().is_some() {
        link_recovery_reason.take();
        true
    } else {
        false
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActiveCaptureRecoveryFact {
    ParseRejected,
    NotificationOnly,
    CurrentSegmentStarted,
    CurrentSegmentPcm,
    CurrentSegmentTerminal,
    IgnoredPacket(crate::embedded_audio::IgnoredPacketReason),
}

/// Classify what the collector actually established after a transport
/// notification arrived.  This intentionally does not alter recovery policy:
/// it separates link-level arrival from current-segment progress for tests and
/// diagnostic logs.
fn active_capture_recovery_fact(
    parse_succeeded: bool,
    event: Option<&crate::embedded_audio::SessionEvent>,
) -> ActiveCaptureRecoveryFact {
    if !parse_succeeded {
        return ActiveCaptureRecoveryFact::ParseRejected;
    }
    match event {
        Some(crate::embedded_audio::SessionEvent::Started { .. }) => {
            ActiveCaptureRecoveryFact::CurrentSegmentStarted
        }
        Some(crate::embedded_audio::SessionEvent::AudioData { .. }) => {
            ActiveCaptureRecoveryFact::CurrentSegmentPcm
        }
        Some(
            crate::embedded_audio::SessionEvent::Stopped { .. }
            | crate::embedded_audio::SessionEvent::Cancelled { .. }
            | crate::embedded_audio::SessionEvent::Error { .. },
        ) => ActiveCaptureRecoveryFact::CurrentSegmentTerminal,
        Some(crate::embedded_audio::SessionEvent::Ignored(reason)) => {
            ActiveCaptureRecoveryFact::IgnoredPacket(*reason)
        }
        None => ActiveCaptureRecoveryFact::NotificationOnly,
    }
}

/// Windows can deliver a queued `ConnectionStatusChanged(Disconnected)` callback
/// while the GATT session is still active and an audio capture is receiving
/// packets. Treat that callback as advisory for the active capture: a real
/// loss will be detected by the heartbeat/notification watchdog, while a
/// transient WinRT status flap must not abort a live recording after 5 seconds.
fn stale_disconnect_signal_after_reconnect(
    cleanup: &NotifyCleanup,
    collector: &crate::embedded_audio::SessionCollector,
) -> bool {
    let device_connected = cleanup.target.device.as_ref().is_some_and(|device| {
        device
            .ConnectionStatus()
            .is_ok_and(|status| status == BluetoothConnectionStatus::Connected)
    });
    let session_active = cleanup.target.session.as_ref().is_some_and(|session| {
        session
            .SessionStatus()
            .is_ok_and(|status| status == GattSessionStatus::Active)
    });
    should_ignore_stale_disconnect_status(
        device_connected,
        session_active,
        collector_has_active_recoverable_session(collector),
    )
}

pub fn capture_notification_events(
    timeout: Duration,
    on_event: &mut crate::embedded_ble::BleNotificationHandler<'_>,
) -> Result<(), String> {
    let mut on_ready = || Ok(());
    capture_notification_events_until_cancelled(
        Some(timeout),
        Arc::new(AtomicBool::new(false)),
        &mut on_ready,
        on_event,
    )
}

pub fn capture_notification_events_until_cancelled(
    idle_timeout: Option<Duration>,
    cancel_requested: Arc<AtomicBool>,
    on_ready: &mut crate::embedded_ble::BleReadyHandler<'_>,
    on_event: &mut crate::embedded_ble::BleNotificationHandler<'_>,
) -> Result<(), String> {
    capture_notification_events_until_cancelled_impl(
        idle_timeout,
        cancel_requested,
        on_ready,
        on_event,
        CaptureTerminalBehavior::StopCapture,
        None,
    )
}

pub fn capture_notification_events_continuous_until_cancelled(
    idle_timeout: Option<Duration>,
    cancel_requested: Arc<AtomicBool>,
    on_ready: &mut crate::embedded_ble::BleReadyHandler<'_>,
    on_event: &mut crate::embedded_ble::BleNotificationHandler<'_>,
) -> Result<(), String> {
    capture_notification_events_until_cancelled_impl(
        idle_timeout,
        cancel_requested,
        on_ready,
        on_event,
        CaptureTerminalBehavior::ContinueListening,
        None,
    )
}

pub fn capture_notification_events_continuous_with_connection_handoff_until_cancelled(
    idle_timeout: Option<Duration>,
    cancel_requested: Arc<AtomicBool>,
    leave_notify_cccd_enabled_on_cancel: Arc<AtomicBool>,
    on_ready: &mut crate::embedded_ble::BleReadyHandler<'_>,
    on_event: &mut crate::embedded_ble::BleNotificationHandler<'_>,
) -> Result<(), String> {
    capture_notification_events_until_cancelled_impl(
        idle_timeout,
        cancel_requested,
        on_ready,
        on_event,
        CaptureTerminalBehavior::ContinueListening,
        Some(leave_notify_cccd_enabled_on_cancel),
    )
}

fn capture_notification_events_until_cancelled_impl(
    idle_timeout: Option<Duration>,
    cancel_requested: Arc<AtomicBool>,
    on_ready: &mut crate::embedded_ble::BleReadyHandler<'_>,
    on_event: &mut crate::embedded_ble::BleNotificationHandler<'_>,
    terminal_behavior: CaptureTerminalBehavior,
    leave_notify_cccd_enabled_on_cancel: Option<Arc<AtomicBool>>,
) -> Result<(), String> {
    let _cancel_scope = NotifyCaptureCancelScope::install(&cancel_requested);
    if notify_capture_cancel_requested() {
        return Err(notify_capture_cancelled_error("notify capture open"));
    }
    let capture_guard = BleCaptureGuard::enter(idle_timeout)?;
    let capture_id = capture_guard.session_id();
    let pipeline_capture = crate::observability::begin_embedded_audio_pipeline_capture(capture_id);
    let pipeline_observation = pipeline_capture.observation();
    let ingress_diagnostic = BleIngressDiagnosticSink::for_capture(capture_id);
    if terminal_behavior == CaptureTerminalBehavior::ContinueListening
        && ble_ota_process_mutex_busy()
    {
        log::info!(
            "[embedded-ble] capture #{capture_id}: BLE OTA operation active in another process; deferring background listener before notify open"
        );
        return Err(background_listener_deferred_for_ota_error());
    }
    let target = open_notify_target_with_retry(capture_id)?;
    wait_post_ota_notify_target_connected(capture_id, &target)?;
    let max_pdu_size = target
        .session
        .as_ref()
        .and_then(|session| session.MaxPduSize().ok());
    log::info!(
        "[embedded-ble] capture #{capture_id}: notify target ready max_pdu_size={max_pdu_size:?}"
    );
    let characteristic = target.characteristic.clone();
    let (tx, rx) = mpsc::channel::<BleCaptureSignal>();
    let (control_tx, control_rx) = mpsc::channel::<AudioControlRequest>();
    let notification_tx = tx.clone();
    let notification_log_count = Arc::new(AtomicUsize::new(0));
    let notification_log_count_for_handler = Arc::clone(&notification_log_count);
    let pipeline_observation_for_handler = pipeline_observation.clone();
    let ingress_diagnostic_for_handler = ingress_diagnostic.clone();
    let handler = TypedEventHandler::<GattCharacteristic, GattValueChangedEventArgs>::new(
        move |_sender, args| {
            if let Some(args) = args {
                if let Ok(buffer) = args.CharacteristicValue() {
                    if let Ok(bytes) = buffer_to_vec(&buffer) {
                        pipeline_observation_for_handler.record_raw_notification(bytes.len());
                        if let Some(diagnostic) = ingress_diagnostic_for_handler.as_ref() {
                            diagnostic.record_received(&bytes);
                        }
                        let log_index =
                            notification_log_count_for_handler.fetch_add(1, Ordering::Relaxed);
                        if log_index < CAPTURE_NOTIFICATION_INFO_LOG_LIMIT {
                            let prefix_len = bytes.len().min(4);
                            log::info!(
                                "[embedded-ble] capture #{capture_id}: notification #{} bytes={} prefix={:02X?}",
                                log_index + 1,
                                bytes.len(),
                                &bytes[..prefix_len]
                            );
                        }
                        let sent = notification_tx.send(BleCaptureSignal::Notification(bytes));
                        pipeline_observation_for_handler.record_channel_forward(sent.is_ok());
                    }
                }
            }
            Ok(())
        },
    );

    let mut cleanup = NotifyCleanup::new(capture_id, target);
    if cleanup.target.control.is_some() {
        cleanup.set_audio_control_registration(ActiveAudioControlRegistration::install(
            capture_id, control_tx,
        ));
    } else {
        log::warn!(
            "[embedded-ble] capture #{capture_id}: audio control unavailable for active capture"
        );
    }
    let token = characteristic
        .ValueChanged(&handler)
        .map_err(|err| format!("BLE ValueChanged handler registration failed: {err}"))?;
    cleanup.set_token(token);
    log::info!("[embedded-ble] capture #{capture_id}: ValueChanged handler registered");
    let connection_token = cleanup.target.device.as_ref().and_then(|device| {
        register_device_connection_status_handler(capture_id, device, tx.clone())
    });
    if let Some(token) = connection_token {
        cleanup.set_connection_status_token(token);
    }
    let session_token = cleanup.target.session.as_ref().and_then(|session| {
        register_gatt_session_status_handler(capture_id, session, tx.clone())
    });
    if let Some(token) = session_token {
        cleanup.set_session_status_token(token);
    }
    #[cfg(debug_assertions)]
    register_validation_disconnect_injection_handler(
        capture_id,
        tx.clone(),
        Arc::clone(&cancel_requested),
    );

    if notify_cccd_prereset_required(terminal_behavior) {
        log::info!("[embedded-ble] capture #{capture_id}: resetting notify CCCD before enable");
        match write_cccd_with_timeout(
            &characteristic,
            GattClientCharacteristicConfigurationDescriptorValue::None,
            Duration::from_secs(2),
        ) {
            Ok(status) => {
                log::info!(
                    "[embedded-ble] capture #{capture_id}: notify CCCD reset status={status:?}"
                )
            }
            Err(err) => {
                if let Some(recovery_error) =
                    cccd_notify_recovery_pairing_error(&err, cleanup.target.bluetooth_address)
                {
                    log::warn!(
                        "[embedded-ble] capture #{capture_id}: notify CCCD reset hit recovery pairing window; entering Type PairAsync recovery before notify-enable retries: {err}"
                    );
                    return Err(recovery_error);
                }
                log::warn!(
                    "[embedded-ble] capture #{capture_id}: notify CCCD reset skipped: {err}"
                )
            }
        }
        std::thread::sleep(Duration::from_millis(150));
    } else {
        log::info!(
            "[embedded-ble] capture #{capture_id}: continuous listener enables notify CCCD without pre-reset"
        );
    }
    log::info!("[embedded-ble] capture #{capture_id}: enabling notify CCCD");
    let status = if cleanup.target.post_ota_preserved_cccd {
        // The controlled OTA handoff deliberately leaves CCCD enabled. Windows
        // restores that subscription during the bonded reconnect before it
        // exposes the post-boot GATT service. Rewriting the already-enabled
        // descriptor queues behind Windows' stale pre-reboot ATT request and
        // can block for its native ~30 s deadline.
        log::info!(
            "[embedded-ble] capture #{capture_id}: reusing Windows-restored notify CCCD after verified OTA reconnect"
        );
        GattCommunicationStatus::Success
    } else {
        write_cccd_notify_with_retry(
            capture_id,
            "capture",
            &characteristic,
            CCCD_ENABLE_TIMEOUT,
            cleanup.target.bluetooth_address,
        )?
    };
    if status != GattCommunicationStatus::Success {
        return Err(format!("BLE CCCD notify write returned status={status:?}"));
    }
    log::info!("[embedded-ble] capture #{capture_id}: notify CCCD enabled");
    crate::startup_evidence::record_startup_stage("notify_cccd_enabled");
    let type_heartbeat_enabled =
        type_heartbeat_enabled_for_terminal_behavior(terminal_behavior);
    let mut next_type_heartbeat = None;
    // Notify subscription alone is not the recovery terminal state. The
    // firmware keeps its pairing/LED recovery window open until it has
    // accepted TYPE:READY (or a later Type heartbeat after a retry).
    let mut type_ready_confirmed = false;
    let type_ready_command = type_ready_command_bytes();
    if type_heartbeat_enabled && ble_ota_process_mutex_busy() {
        log::info!(
            "[embedded-ble] capture #{capture_id}: BLE OTA operation active in another process; closing background listener before Type heartbeat ready"
        );
        cleanup.disable_notify();
        return Err(background_listener_deferred_for_ota_error());
    }
    match cleanup.write_type_heartbeat(&type_ready_command, "Type heartbeat ready") {
        Ok(()) => {
            crate::startup_evidence::record_startup_stage("type_ready_written");
            cleanup.mark_type_heartbeat_open();
            release_listener_ota_post_confirm_device();
            if let Some(address) = cleanup.target.bluetooth_address {
                persist_successful_notify_target_address(address, "Type heartbeat ready");
            }
            if let Err(err) = cleanup.write_type_heartbeat(
                b"TYPE:AUDIO:LOSSLESS_RICE:3\n",
                "Type lossless audio capability",
            ) {
                log::warn!(
                    "[embedded-ble] capture #{capture_id}: lossless audio capability was not acknowledged; firmware will retain raw PCM: {err}"
                );
            } else {
                log::info!(
                    "[embedded-ble] capture #{capture_id}: lossless audio capability announced"
                );
            }
            on_ready()?;
            type_ready_confirmed = true;
            if type_heartbeat_enabled {
                next_type_heartbeat = Some(Instant::now() + TYPE_HEARTBEAT_INTERVAL);
            }
        }
        Err(err) => {
            if cleanup.target.post_ota_preserved_cccd {
                log::warn!(
                    "[embedded-ble] capture #{capture_id}: post-OTA TYPE:READY writability probe failed; abandoning poisoned target for a fresh retry: {err}"
                );
                cleanup.abandon_poisoned_post_ota_target();
                return Err(format!(
                    "post-OTA TYPE:READY target was connected but not ATT-writable: {err}"
                ));
            }
            if type_heartbeat_enabled {
                log::warn!(
                    "[embedded-ble] capture #{capture_id}: Type heartbeat ready failed; keeping notify open for retry: {err}"
                );
                next_type_heartbeat = Some(Instant::now() + TYPE_HEARTBEAT_INTERVAL);
            } else {
                log::warn!(
                    "[embedded-ble] capture #{capture_id}: Type ready failed; waiting for audio notifications anyway: {err}"
                );
            }
        }
    }

    let deadline = idle_timeout.map(|timeout| Instant::now() + timeout);
    let mut collector = crate::embedded_audio::SessionCollector::default();
    let mut stop_drain_deadline: Option<Instant> = None;
    let mut link_recovery_deadline: Option<Instant> = None;
    let mut link_recovery_reason: Option<String> = None;
    let mut notify_refresh_required_after_link_recovery = false;
    let mut ec11_recovery_prepare_disconnect_deadline: Option<Instant> = None;
    let mut ec11_recovery_disconnect_deadline: Option<Instant> = None;
    let mut consecutive_type_heartbeat_failures = 0u32;
    let mut next_pipeline_snapshot_at = Instant::now();
    loop {
        cleanup.drain_audio_control_requests(&control_rx);
        let now = Instant::now();
        if now >= next_pipeline_snapshot_at {
            pipeline_observation.snapshot("capture_poll", false);
            next_pipeline_snapshot_at = now + Duration::from_secs(1);
        }
        if type_heartbeat_enabled
            && !collector_has_active_recoverable_session(&collector)
            && ble_ota_process_mutex_busy()
        {
            log::info!(
                "[embedded-ble] capture #{capture_id}: BLE OTA operation active in another process; closing idle background listener"
            );
            cleanup.disable_notify();
            return Err(background_listener_deferred_for_ota_error());
        }
        if let Some(due) = next_type_heartbeat {
            if now >= due {
                if let Err(err) = cleanup.write_type_heartbeat(b"TYPE:HB\n", "Type heartbeat") {
                    consecutive_type_heartbeat_failures =
                        consecutive_type_heartbeat_failures.saturating_add(1);
                    let reason = format!("{err}; BLE audio/control response missing");
                    if collector_has_active_recoverable_session(&collector) {
                        let stats = collector.stats();
                        if link_recovery_deadline.is_none() {
                            link_recovery_deadline =
                                Some(now + ACTIVE_CAPTURE_LINK_RECOVERY_TIMEOUT);
                            link_recovery_reason = Some(reason.clone());
                            log::warn!(
                                "[embedded-ble] capture #{capture_id}: {reason}; waiting for audio notify recovery (session_id={:?}, packets={}, timeout_ms={})",
                                stats.session_id,
                                stats.received_packet_count,
                                ACTIVE_CAPTURE_LINK_RECOVERY_TIMEOUT.as_millis()
                            );
                        } else {
                            log::warn!(
                                "[embedded-ble] capture #{capture_id}: additional heartbeat failure while waiting for recovery: {reason}"
                            );
                        }
                    } else {
                        let log_message =
                            format!("[embedded-ble] capture #{capture_id}: {reason}; keeping idle notify open for heartbeat retry");
                        if consecutive_type_heartbeat_failures == 1 {
                            log::info!("{log_message}");
                        } else {
                            log::warn!(
                                "{log_message}; consecutive_failures={consecutive_type_heartbeat_failures}"
                            );
                        }
                    }
                } else {
                    if consecutive_type_heartbeat_failures > 0 {
                        log::info!(
                            "[embedded-ble] capture #{capture_id}: Type heartbeat recovered after {consecutive_type_heartbeat_failures} failure(s)"
                        );
                        if should_restore_lossless_after_heartbeat(
                            consecutive_type_heartbeat_failures,
                            collector_has_active_recoverable_session(&collector),
                        ) {
                            if let Err(err) = cleanup.write_type_heartbeat(
                                b"TYPE:AUDIO:LOSSLESS_RICE:3\n",
                                "Type lossless audio capability recovery",
                            ) {
                                log::warn!(
                                    "[embedded-ble] capture #{capture_id}: lossless audio capability recovery was not acknowledged; firmware will retain raw PCM: {err}"
                                );
                            } else {
                                log::info!(
                                    "[embedded-ble] capture #{capture_id}: lossless audio capability restored after heartbeat recovery"
                                );
                            }
                        }
                    }
                    consecutive_type_heartbeat_failures = 0;
                    cleanup.mark_type_heartbeat_open();
                    if !type_ready_confirmed {
                        on_ready()?;
                        type_ready_confirmed = true;
                        log::info!(
                            "[embedded-ble] capture #{capture_id}: Type ready terminal confirmation recovered through heartbeat"
                        );
                    }
                }
                next_type_heartbeat = Some(now + TYPE_HEARTBEAT_INTERVAL);
            }
        }
        if cancel_requested.load(Ordering::SeqCst) {
            log::info!(
                "[embedded-ble] capture #{capture_id}: cancelled by caller; closing notify"
            );
            cleanup.finish_after_caller_cancel(
                terminal_behavior,
                leave_notify_cccd_enabled_on_cancel
                    .as_ref()
                    .is_some_and(|handoff| handoff.load(Ordering::SeqCst)),
            );
            return Ok(());
        }
        if deadline.is_some_and(|deadline| now >= deadline) {
            cleanup.log_embedded_audio_status_snapshot("capture timeout");
            return Err(format!(
                "BLE embedded audio capture timed out after {} ms",
                idle_timeout
                    .expect("deadline exists when timeout is reported")
                    .as_millis()
            ));
        }
        if stop_drain_deadline.is_some_and(|drain_deadline| now >= drain_deadline) {
            let stats = collector.stats();
            let reason = super::stop_drain_timeout_reason(&stats);
            log::warn!("[embedded-ble] {reason}");
            // Continuous background: a short post-STOP drain miss must not TYPE:BYE /
            // CCCD-off. Finalize the local collector and keep the notify subscription.
            if terminal_behavior == CaptureTerminalBehavior::ContinueListening {
                log::warn!(
                    "[embedded-ble] capture #{capture_id}: stop-drain timeout while continuous listening; keeping notify open (session_id={:?}, received={}, missing={})",
                    stats.session_id,
                    stats.received_packet_count,
                    stats.missing_packet_count
                );
                if should_refresh_notify_after_link_recovery(
                    notify_refresh_required_after_link_recovery,
                    terminal_behavior,
                ) {
                    log::warn!(
                        "[embedded-ble] capture #{capture_id}: recovered link reached a stop-drain terminal boundary; reopening a fresh notify/GATT target before the next recording"
                    );
                    cleanup.defer_type_heartbeat_bye_until_processing_done();
                    cleanup.disable_notify();
                    return Ok(());
                }
                collector.reset();
                stop_drain_deadline = None;
                continue;
            }
            cleanup.disable_notify();
            return if crate::embedded_audio::transport_v1::stop_drain_expired_finalizes(
                &collector,
            ) {
                Ok(())
            } else {
                Err(reason)
            };
        }
        if active_capture_link_recovery_expired(link_recovery_deadline, now) {
            let reason = link_recovery_reason
                .as_deref()
                .unwrap_or("BLE link recovery timed out");
            let message = format!(
                "BLE embedded audio capture link recovery timed out after {} ms: {reason}",
                ACTIVE_CAPTURE_LINK_RECOVERY_TIMEOUT.as_millis()
            );
            log::warn!("[embedded-ble] capture #{capture_id}: {message}");
            cleanup.disable_notify();
            return Err(message);
        }
        if ec11_recovery_prepare_disconnect_deadline
            .is_some_and(|prepare_deadline| now >= prepare_deadline)
        {
            log::info!(
                "[embedded-ble] capture #{capture_id}: EC11 recovery pre-authorization expired without a firmware disconnect"
            );
            ec11_recovery_prepare_disconnect_deadline = None;
        }
        if ec11_recovery_disconnect_deadline
            .is_some_and(|disconnect_deadline| now >= disconnect_deadline)
        {
            let message = format!(
                "Listener EC11 hardware recovery notice did not receive the expected firmware disconnect within {} ms",
                EC11_HARDWARE_RECOVERY_DISCONNECT_TIMEOUT.as_millis()
            );
            log::warn!("[embedded-ble] capture #{capture_id}: {message}");
            cleanup.defer_type_heartbeat_bye_until_processing_done();
            cleanup.finish(NotifyCccdTeardown::LeaveEnabled);
            return Err(message);
        }
        let receive_timeout = [
            stop_drain_deadline,
            deadline,
            link_recovery_deadline,
            ec11_recovery_prepare_disconnect_deadline,
            ec11_recovery_disconnect_deadline,
        ]
        .into_iter()
        .flatten()
        .chain(next_type_heartbeat)
        .map(|deadline| deadline.saturating_duration_since(now))
        .min()
        .unwrap_or(RECEIVE_POLL_INTERVAL)
        .min(RECEIVE_POLL_INTERVAL);
        let signal = match rx.recv_timeout(receive_timeout) {
            Ok(signal) => signal,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let now = Instant::now();
                if cancel_requested.load(Ordering::SeqCst) {
                    log::info!(
                        "[embedded-ble] capture #{capture_id}: cancelled by caller; closing notify"
                    );
                    cleanup.finish_after_caller_cancel(
                        terminal_behavior,
                        leave_notify_cccd_enabled_on_cancel
                            .as_ref()
                            .is_some_and(|handoff| handoff.load(Ordering::SeqCst)),
                    );
                    return Ok(());
                }
                if stop_drain_deadline.is_some_and(|drain_deadline| now >= drain_deadline) {
                    let stats = collector.stats();
                    let reason = super::stop_drain_timeout_reason(&stats);
                    log::warn!("[embedded-ble] {reason}");
                    if terminal_behavior == CaptureTerminalBehavior::ContinueListening {
                        log::warn!(
                            "[embedded-ble] capture #{capture_id}: stop-drain timeout while continuous listening; keeping notify open (session_id={:?}, received={}, missing={})",
                            stats.session_id,
                            stats.received_packet_count,
                            stats.missing_packet_count
                        );
                        if should_refresh_notify_after_link_recovery(
                            notify_refresh_required_after_link_recovery,
                            terminal_behavior,
                        ) {
                            log::warn!(
                                "[embedded-ble] capture #{capture_id}: recovered link reached a stop-drain terminal boundary; reopening a fresh notify/GATT target before the next recording"
                            );
                            cleanup.defer_type_heartbeat_bye_until_processing_done();
                            cleanup.disable_notify();
                            return Ok(());
                        }
                        collector.reset();
                        stop_drain_deadline = None;
                        continue;
                    }
                    cleanup.disable_notify();
                    return if crate::embedded_audio::transport_v1::stop_drain_expired_finalizes(
                        &collector,
                    ) {
                        Ok(())
                    } else {
                        Err(reason)
                    };
                }
                if active_capture_link_recovery_expired(link_recovery_deadline, now) {
                    let reason = link_recovery_reason
                        .as_deref()
                        .unwrap_or("BLE link recovery timed out");
                    let message = format!(
                        "BLE embedded audio capture link recovery timed out after {} ms: {reason}",
                        ACTIVE_CAPTURE_LINK_RECOVERY_TIMEOUT.as_millis()
                    );
                    log::warn!("[embedded-ble] capture #{capture_id}: {message}");
                    cleanup.disable_notify();
                    return Err(message);
                }
                continue;
            }
            Err(err) => {
                return Err(format!(
                    "BLE embedded audio notification wait failed: {err}"
                ));
            }
        };
        let (notification, link_recovery_notification_pending) = match signal {
            BleCaptureSignal::Notification(notification) => {
                (
                    notification,
                    active_capture_link_recovery_notification_pending(link_recovery_deadline),
                )
            }
            BleCaptureSignal::GattSessionInactive(reason) => {
                log::warn!(
                    "[embedded-ble] capture #{capture_id}: {reason}; retaining the notify target because GATT Inactive is advisory until the device disconnects or heartbeat fails"
                );
                continue;
            }
            BleCaptureSignal::GattSessionActive => {
                log::info!(
                    "[embedded-ble] capture #{capture_id}: GATT session returned to Active while retaining the notify target"
                );
                continue;
            }
            BleCaptureSignal::Disconnected(reason) => {
                if reason.contains("device connection status changed to Disconnected")
                    && stale_disconnect_signal_after_reconnect(&cleanup, &collector)
                {
                    log::info!(
                        "[embedded-ble] capture #{capture_id}: ignoring stale Disconnected event because the device and GATT session are connected/active again"
                    );
                    continue;
                }
                if ec11_recovery_disconnect_deadline.take().is_some() {
                    log::info!(
                        "[embedded-ble] capture #{capture_id}: firmware disconnect observed after EC11 recovery notice; releasing the retained GATT session for recovery arbitration"
                    );
                    cleanup.defer_type_heartbeat_bye_until_processing_done();
                    cleanup.finish(NotifyCccdTeardown::LeaveEnabled);
                    return Err(
                        "Listener EC11 hardware recovery notice received before pairing reset; Type observed the firmware disconnect and must scan the matching recovery advertisement and run automatic PairAsync recovery".to_string(),
                    );
                }
                if ec11_recovery_prepare_disconnect_deadline.take().is_some() {
                    log::info!(
                        "[embedded-ble] capture #{capture_id}: firmware disconnect observed during EC11 pre-authorized double-click window; recovery remains gated on matching advertising"
                    );
                    cleanup.defer_type_heartbeat_bye_until_processing_done();
                    cleanup.finish(NotifyCccdTeardown::LeaveEnabled);
                    return Err(
                        "Listener EC11 hardware recovery notice received before pairing reset via pre-authorization; Type must scan the matching recovery advertisement before automatic PairAsync recovery".to_string(),
                    );
                }
                if collector_has_active_recoverable_session(&collector) {
                    if let Some(recovery_error) =
                        active_capture_disconnect_recovery_pairing_error(&reason)
                    {
                        let stats = collector.stats();
                        log::warn!(
                            "[embedded-ble] capture #{capture_id}: recovery advertising proves the active session cannot resume; entering Type PairAsync recovery without the active-session wait (session_id={:?}, packets={})",
                            stats.session_id,
                            stats.received_packet_count,
                        );
                        cleanup.disable_notify();
                        return Err(recovery_error);
                    }
                    let stats = collector.stats();
                    if link_recovery_deadline.is_none() {
                        link_recovery_deadline =
                            Some(Instant::now() + ACTIVE_CAPTURE_LINK_RECOVERY_TIMEOUT);
                        link_recovery_reason = Some(reason.clone());
                        log::warn!(
                            "[embedded-ble] capture #{capture_id}: {reason}; keeping notify open for active session recovery (session_id={:?}, packets={}, timeout_ms={})",
                            stats.session_id,
                            stats.received_packet_count,
                            ACTIVE_CAPTURE_LINK_RECOVERY_TIMEOUT.as_millis()
                        );
                    } else {
                        log::warn!(
                            "[embedded-ble] capture #{capture_id}: additional active-session disconnect while waiting for recovery: {reason}"
                        );
                    }
                    continue;
                }
                log::warn!("[embedded-ble] capture #{capture_id}: {reason}");
                cleanup.disable_notify();
                return Err(reason);
            }
        };
        if super::is_ec11_hardware_recovery_prepare_notice(&notification) {
            log::info!(
                "[embedded-ble] capture #{capture_id}: received EC11 recovery pre-authorization during the double-click window"
            );
            if let Err(err) = cleanup.write_ec11_recovery_prepare_acknowledgement() {
                log::warn!(
                    "[embedded-ble] capture #{capture_id}: EC11 recovery pre-authorization acknowledgement was not queued: {err}"
                );
                continue;
            }
            log::info!(
                "[embedded-ble] capture #{capture_id}: EC11 recovery pre-authorization acknowledgement queued via active GATT control"
            );
            ec11_recovery_prepare_disconnect_deadline =
                Some(Instant::now() + EC11_HARDWARE_RECOVERY_PREPARE_TIMEOUT);
            continue;
        }
        if super::is_ec11_hardware_recovery_notice(&notification) {
            log::warn!(
                "[embedded-ble] capture #{capture_id}: received EC11 hardware recovery notice; retaining the GATT session until the firmware disconnect completes"
            );
            if let Err(err) = cleanup.write_ec11_recovery_acknowledgement() {
                log::warn!(
                    "[embedded-ble] capture #{capture_id}: EC11 recovery notice acknowledgement was not queued; retaining the existing GATT session without PairAsync authorization: {err}"
                );
                continue;
            }
            log::info!(
                "[embedded-ble] capture #{capture_id}: EC11 recovery acknowledgement queued via active GATT control"
            );
            cleanup.defer_type_heartbeat_bye_until_processing_done();
            ec11_recovery_disconnect_deadline =
                Some(Instant::now() + EC11_HARDWARE_RECOVERY_DISCONNECT_TIMEOUT);
            continue;
        }
        let raw_notification_bytes = notification.len();
        let notification = match crate::audio_transport_codec::normalize_listener_audio_notification(
            &notification,
        ) {
            Ok(notification) => {
                pipeline_observation
                    .record_normalize(raw_notification_bytes, Some(notification.len()));
                notification
            }
            Err(err) => {
                pipeline_observation.record_normalize(raw_notification_bytes, None);
                if link_recovery_notification_pending {
                    log::warn!(
                        "[embedded-ble] capture #{capture_id}: active-session recovery notification qualification fact=ParseRejected"
                    );
                }
                return Err(format!(
                    "[embedded-ble] capture #{capture_id}: lossless audio notification rejected: {err}"
                ));
            }
        };
        let terminal = super::is_terminal_notification(&notification);
        let collector_result = collector.handle_notification(&notification);
        if collector_result.is_err() {
            pipeline_observation.record_capture_parse_reject();
        }
        if let Some(event) = collector_result.as_ref().ok() {
            pipeline_observation.record_capture_event(event);
        }
        if let Some(diagnostic) = ingress_diagnostic.as_ref() {
            diagnostic.record_collector(
                capture_id,
                collector.last_admission_fact(),
                &notification,
            );
        }
        let recovery_fact = active_capture_recovery_fact(
            collector_result.is_ok(),
            collector_result.as_ref().ok(),
        );
        let link_recovered_after_valid_pcm = link_recovery_notification_pending
            && recovery_fact == ActiveCaptureRecoveryFact::CurrentSegmentPcm
            && take_active_capture_link_recovery_after_valid_pcm(
                &mut link_recovery_deadline,
                &mut notify_refresh_required_after_link_recovery,
            );
        if link_recovered_after_valid_pcm {
            log::info!(
                "[embedded-ble] capture #{capture_id}: link recovered after active-session disconnect through current-segment PCM; current recording may finish, but this notify/GATT target is quarantined: {}",
                link_recovery_reason
                    .take()
                    .unwrap_or_else(|| "unknown".to_string())
            );
        } else if link_recovery_notification_pending {
            log::warn!(
                "[embedded-ble] capture #{capture_id}: active-session recovery notification qualification fact={recovery_fact:?}"
            );
        }
        let local_event = collector_result.ok();
        let capture_admission_fact = collector.last_admission_fact();
        let capture_admission_receipt = collector.last_admission_receipt();
        on_event(crate::embedded_ble::BleNotificationEvent {
            notification,
            terminal,
            capture_generation: capture_id,
            capture_admission_fact,
            capture_admission_receipt,
        })?;
        if matches!(
            local_event,
            Some(crate::embedded_audio::SessionEvent::Cancelled { .. })
                | Some(crate::embedded_audio::SessionEvent::Error { .. })
        ) {
            let recovery_wait_ended_at_terminal = clear_active_capture_link_recovery_wait(
                &mut link_recovery_deadline,
                &mut link_recovery_reason,
            );
            if recovery_wait_ended_at_terminal {
                log::info!(
                    "[embedded-ble] capture #{capture_id}: active-session recovery wait ended at cancel/error terminal boundary; no audio recovery was claimed"
                );
            }
            if terminal_behavior == CaptureTerminalBehavior::ContinueListening {
                if should_refresh_notify_after_link_recovery(
                    notify_refresh_required_after_link_recovery,
                    terminal_behavior,
                ) {
                    log::warn!(
                        "[embedded-ble] capture #{capture_id}: recovered link reached a cancel/error terminal boundary; reopening a fresh notify/GATT target before the next recording"
                    );
                    cleanup.defer_type_heartbeat_bye_until_processing_done();
                    cleanup.disable_notify();
                    return Ok(());
                }
                log::info!(
                    "[embedded-ble] capture #{capture_id}: terminal cancel/error while continuous listening; keeping notify open for the next session"
                );
                collector.reset();
                stop_drain_deadline = None;
                continue;
            }
            cleanup.disable_notify();
            return Ok(());
        }
        if matches!(
            local_event,
            Some(crate::embedded_audio::SessionEvent::Stopped { .. })
        ) {
            let recovery_wait_ended_at_terminal = clear_active_capture_link_recovery_wait(
                &mut link_recovery_deadline,
                &mut link_recovery_reason,
            );
            if recovery_wait_ended_at_terminal {
                log::info!(
                    "[embedded-ble] capture #{capture_id}: active-session recovery wait ended at STOP boundary; stop-drain owns the remaining tail"
                );
            }
            stop_drain_deadline = Some(Instant::now() + super::STOP_DRAIN_TIMEOUT);
        }
        if collector.has_successful_complete_session() {
            if terminal_behavior == CaptureTerminalBehavior::ContinueListening {
                let stats = collector.stats();
                if should_refresh_notify_after_link_recovery(
                    notify_refresh_required_after_link_recovery,
                    terminal_behavior,
                ) {
                    log::warn!(
                        "[embedded-ble] capture #{capture_id}: recovered link completed its preserved recording; reopening a fresh notify/GATT target before the next recording (session_id={:?}, pcm_bytes={}, packets={})",
                        stats.session_id,
                        stats.received_pcm_bytes,
                        stats.received_packet_count
                    );
                    cleanup.defer_type_heartbeat_bye_until_processing_done();
                    cleanup.disable_notify();
                    return Ok(());
                }
                log::info!(
                    "[embedded-ble] capture #{capture_id}: complete session received; keeping notify open for background listener (session_id={:?}, pcm_bytes={}, packets={})",
                    stats.session_id,
                    stats.received_pcm_bytes,
                    stats.received_packet_count
                );
                collector.reset();
                stop_drain_deadline = None;
                continue;
            }
            cleanup.defer_type_heartbeat_bye_until_processing_done();
            cleanup.disable_notify();
            return Ok(());
        }
        if collector.terminal_received() && stop_drain_deadline.is_some() {
            stop_drain_deadline = Some(Instant::now() + super::STOP_DRAIN_TIMEOUT);
        }
    }
}
