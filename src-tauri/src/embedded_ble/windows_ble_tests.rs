//! Path-separated unit tests for `embedded_ble::windows_ble`.
//! Loaded via `#[path = "windows_ble_tests.rs"]` from `windows_ble.rs`.

use super::*;

macro_rules! include_str {
    ("embedded_ble.rs") => {{
        concat!(
            std::include_str!("mod.rs"),
            "\n",
            std::include_str!("windows_ble/mod.rs"),
            "\n",
            std::include_str!("windows_ble/ota_transfer.rs"),
            "\n",
            std::include_str!("windows_ble/pairing.rs"),
            "\n",
            std::include_str!("windows_ble/pnp_cache.rs"),
            "\n",
            std::include_str!("windows_ble/recording_control.rs"),
            "\n",
            std::include_str!("windows_ble/capture_events.rs"),
            "\n",
            std::include_str!("windows_ble/notify_open.rs"),
            "\n",
            std::include_str!("windows_ble/unpair.rs"),
            "\n",
            std::include_str!("windows_ble/ota_open.rs"),
            "\n",
            std::include_str!("windows_ble/gatt_open.rs")
        )
        .replace("\r\n", "\n")
    }};
}

fn active_audio_control_test_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn bluetooth_target_name_test_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[test]
fn ota_wwr_depth_stays_within_the_device_acl_pool() {
    assert_eq!(LISTENER_OTA_V1_WWR_PIPELINE_DEPTH, 40);
    assert_eq!(LISTENER_OTA_V1_CHUNK_PAYLOAD_BYTES, 443);
    assert!(
        LISTENER_OTA_V1_WWR_PIPELINE_DEPTH * 2 <= 128 - 48,
        "the link-aligned 443-byte OTA pipeline must leave 48 of the PSRAM-backed 128-buffer controller ACL pool reserved"
    );
}

#[test]
fn listener_ota_target_open_primes_the_bounded_firmware_prepare_lease() {
    let source = include_str!("embedded_ble.rs");
    assert_eq!(
        source
            .matches("Listener OTA v1 readiness prime completed")
            .count(),
        2,
        "normal and deadline OTA target opens must both prime firmware before BEGIN"
    );
    assert!(
        source.matches("OTA_READINESS_UUID").count() >= 3
            && source
                .matches(
                    "BluetoothCacheMode::Uncached,\n        \"Listener OTA v1 readiness prime\""
                )
                .count()
                == 2,
        "the prepare handshake must read the existing readiness characteristic uncached"
    );
}

#[test]
fn listener_ota_confirms_the_device_link_before_begin() {
    let source = include_str!("embedded_ble.rs");
    assert!(
        source.contains("prime.close(\"before_begin_device_link_convergence\")")
            && source.contains("status.active_link_confirmed()")
            && source.contains("device confirmed active BLE link before BEGIN"),
        "Type must release the temporary WinRT request and await device link confirmation before BEGIN"
    );
}

#[test]
fn listener_ota_rebalances_other_connected_ble_peers_during_bulk() {
    let source = include_str!("embedded_ble.rs");
    assert!(
        source.contains(
            "GetDeviceSelectorFromConnectionStatus(\n        BluetoothConnectionStatus::Connected"
        ) && source.contains("BluetoothLEPreferredConnectionParameters::PowerOptimized()")
            && source.contains("request_concurrent_ble_power_optimized(fresh.bluetooth_address)")
            && source.contains("released concurrent BLE PowerOptimized request"),
        "OTA must temporarily rebalance other connected BLE peers without disconnecting them"
    );
    let rebalance = source
        .find("request_concurrent_ble_power_optimized(fresh.bluetooth_address)")
        .expect("connected BLE peer rebalance should exist");
    let converge = source[rebalance..]
        .find("fresh.converge_active_link_before_begin")
        .map(|offset| rebalance + offset)
        .expect("device active-link convergence should follow peer rebalance");
    let begin = source[converge..]
        .find("transfer_denzic_ota_v1_to_target")
        .map(|offset| converge + offset)
        .expect("bulk transfer should follow active-link convergence");
    assert!(
        rebalance < converge && converge < begin,
        "Windows must receive peer scheduling requests before pre-BEGIN convergence"
    );
    assert!(
        source.contains("let settle_ms = if round == 1 { 150 } else { 700 };"),
        "secure reopen must retain a bounded first-round encryption settle"
    );
}

#[test]
fn listener_ota_bounded_prepare_skips_optional_dis_latency() {
    let source = include_str!("embedded_ble.rs");
    let bounded = source
        .find("let snapshot = if target_prepare_timeout.is_some()")
        .expect("bounded OTA preparation should select a transfer-ready snapshot");
    let transfer_ready = source[bounded..]
        .find("listener_ota_v1_transfer_ready_snapshot_from_target(&target)")
        .expect("bounded OTA preparation should avoid optional identity reads");
    let full_probe = source[bounded..]
        .find("listener_ota_v1_gatt_probe_snapshot_from_target(&target)")
        .expect("ordinary device probes should retain full DIS metadata reads");
    assert!(
        transfer_ready < full_probe,
        "the bounded user transfer path must use the fast service proof"
    );
    assert!(
        source.contains("bounded transfer preparation skipped optional DIS reads"),
        "the fast path should remain explicit and observable"
    );
}

#[test]
fn listener_ota_sync_waits_for_the_controller_air_tail() {
    let source = include_str!("embedded_ble.rs");
    assert!(
        source.contains(
            "const LISTENER_OTA_V1_WWR_AIR_DRAIN_HOLD: Duration = Duration::from_millis(30)"
        ),
        "the fixed 40-write pipeline needs four 7.5 ms events before SYNC"
    );
    let flush = source
        .find("self.flush_pending_wwr()?;")
        .expect("control writes should flush WinRT operations");
    let air_drain = source[flush..]
        .find("std::thread::sleep(LISTENER_OTA_V1_WWR_AIR_DRAIN_HOLD)")
        .map(|offset| flush + offset)
        .expect("SYNC should wait for the controller's queued air tail");
    let control_write = source[air_drain..]
        .find("let result = if use_status_write")
        .map(|offset| air_drain + offset)
        .expect("the response-bearing control write should follow the air drain");
    assert!(flush < air_drain && air_drain < control_write);
}

#[test]
fn background_capture_cancel_scope_is_visible_to_winrt_waits() {
    let cancel = Arc::new(AtomicBool::new(false));
    assert!(!notify_capture_cancel_requested());

    let scope = NotifyCaptureCancelScope::install(&cancel);
    assert!(!notify_capture_cancel_requested());
    cancel.store(true, Ordering::SeqCst);
    assert!(notify_capture_cancel_requested());

    drop(scope);
    assert!(!notify_capture_cancel_requested());
}

#[test]
fn listener_ota_v2_dis_aliases_share_one_package_revision() {
    for revision in [
        "keyboard-v2-n16r8",
        "voice-keyboard-v2-n16r8",
        "esp32s3-wroom-1-n16r8",
    ] {
        assert_eq!(
            normalize_listener_ota_hardware_revision(
                Some("keyboard-v2".to_string()),
                Some(revision.to_string()),
            ),
            Some("keyboard-v2-n16r8".to_string())
        );
    }
    assert_eq!(
        normalize_listener_ota_hardware_revision(
            Some("keyboard-v1".to_string()),
            Some("esp32s3-devkit".to_string()),
        ),
        Some("esp32s3-devkit".to_string())
    );
}

#[test]
fn background_capture_cancellation_interrupts_target_open_before_fallback_scans() {
    let source = include_str!("embedded_ble.rs");
    let open_start = source
        .find("fn open_notify_target()")
        .expect("notify target helper should exist");
    let open_end = source[open_start..]
        .find("fn open_notify_target_with_retry")
        .map(|offset| open_start + offset)
        .expect("notify target helper boundary should exist");
    let open_body = &source[open_start..open_end];
    assert!(
        open_body.contains("if notify_capture_cancel_requested() {\n                    return Err(err);"),
        "a cancelled recent-pairing open must stop before service-selector or advertisement fallbacks"
    );

    let wait_cancel = Arc::new(AtomicBool::new(true));
    let _scope = NotifyCaptureCancelScope::install(&wait_cancel);
    let Ok(operation) = BluetoothLEDevice::FromBluetoothAddressAsync(0x0000_A1B2_C3D4) else {
        return; // WinRT Bluetooth unavailable on this host
    };
    let err = wait_async_operation(operation, Duration::from_secs(30), "cancel probe")
        .expect_err("an installed cancel scope must interrupt the WinRT wait");
    assert!(
        err.contains("cancelled by background listener recovery"),
        "a cancelled wait must surface the background recovery reason: {err}"
    );
}

#[test]
fn continuous_listener_skips_notify_cccd_prereset() {
    assert!(!notify_cccd_prereset_required(
        CaptureTerminalBehavior::ContinueListening
    ));
    assert!(notify_cccd_prereset_required(
        CaptureTerminalBehavior::StopCapture
    ));
}

#[test]
fn type_heartbeat_prefers_no_response_when_available() {
    let both =
        GattCharacteristicProperties::Write | GattCharacteristicProperties::WriteWithoutResponse;
    assert_eq!(
        type_heartbeat_write_option_from_properties(both),
        GattWriteOption::WriteWithoutResponse
    );
    assert_eq!(
        type_heartbeat_write_option_from_properties(GattCharacteristicProperties::Write),
        GattWriteOption::WriteWithResponse
    );
}

#[test]
fn audio_control_toggle_prefers_no_response_when_available() {
    let both =
        GattCharacteristicProperties::Write | GattCharacteristicProperties::WriteWithoutResponse;
    assert_eq!(
        audio_control_write_options_from_properties(both, AudioControlWritePolicy::LowLatency),
        Some((
            GattWriteOption::WriteWithoutResponse,
            Some(GattWriteOption::WriteWithResponse)
        ))
    );
    assert_eq!(
        audio_control_write_options_from_properties(
            GattCharacteristicProperties::Write,
            AudioControlWritePolicy::LowLatency
        ),
        Some((GattWriteOption::WriteWithResponse, None))
    );
    assert_eq!(
        audio_control_write_options_from_properties(
            GattCharacteristicProperties::WriteWithoutResponse,
            AudioControlWritePolicy::LowLatency
        ),
        Some((GattWriteOption::WriteWithoutResponse, None))
    );
}

#[test]
fn recording_stop_prefers_no_response_when_available() {
    assert_eq!(
        audio_control_write_policy(b"VREC:STOP\n"),
        AudioControlWritePolicy::LowLatency
    );
}

#[test]
fn reliable_audio_control_prefers_with_response_when_available() {
    let both =
        GattCharacteristicProperties::Write | GattCharacteristicProperties::WriteWithoutResponse;
    assert_eq!(
        audio_control_write_options_from_properties(both, AudioControlWritePolicy::Reliable),
        Some((
            GattWriteOption::WriteWithResponse,
            Some(GattWriteOption::WriteWithoutResponse)
        ))
    );
    assert_eq!(
        audio_control_write_options_from_properties(
            GattCharacteristicProperties::Write,
            AudioControlWritePolicy::Reliable
        ),
        Some((GattWriteOption::WriteWithResponse, None))
    );
    assert_eq!(
        audio_control_write_options_from_properties(
            GattCharacteristicProperties::WriteWithoutResponse,
            AudioControlWritePolicy::Reliable
        ),
        Some((GattWriteOption::WriteWithoutResponse, None))
    );
}

#[test]
fn ble_advertisement_name_matching_trims_expected_target() {
    assert!(denzic_ble_windows::ble_advertisement_name_matches(
        " Companion ",
        "companion"
    ));
    assert!(denzic_ble_windows::ble_advertisement_name_matches(
        "companion",
        " Companion "
    ));
    assert!(!denzic_ble_windows::ble_advertisement_name_matches(
        "Blistener",
        "companion"
    ));
}

#[test]
fn listener_pairing_name_requires_exact_configured_target() {
    assert!(listener_pairing_name_matches(
        "OfficeType01",
        Some("OfficeType01")
    ));
    assert!(!listener_pairing_name_matches(
        "Blistener",
        Some("OfficeType01")
    ));
    assert!(!listener_pairing_name_matches(
        "listenerB",
        Some("OfficeType01")
    ));
}

#[test]
fn recovery_pairing_prefers_direct_address_before_slow_windows_aep_fallback() {
    let source = include_str!("embedded_ble.rs");
    let start = source
        .find("fn listener_recovery_pairing_candidates")
        .expect("recovery pairing candidate function should exist");
    let body = &source[start..];
    let direct_index = body
        .find("listener_recovery_direct_pairing_candidates")
        .expect("recovery pairing should try direct address candidates");
    let direct_return_index = body
        .find("return Ok(candidates);")
        .expect("recovery pairing should immediately use direct address candidates");
    let selector_index = body
        .find("listener_pairing_candidates_from_unpaired_selector")
        .expect("recovery pairing must keep Windows AEP fallback");

    assert!(
        direct_index < direct_return_index && direct_return_index < selector_index,
        "recovery pairing must attempt fresh direct BLE address candidates immediately instead of waiting on slow Windows AEP fallback"
    );
}

#[test]
fn recovery_pairing_passes_advertised_addresses_into_windows_selector() {
    let source = include_str!("embedded_ble.rs");
    let start = source
        .find("fn listener_recovery_pairing_candidates")
        .expect("recovery pairing candidate function should exist");
    let body = &source[start..];
    let scan_index = body
        .find("scan_listener_pairing_advertisements")
        .expect("recovery pairing must scan advertisements");
    let selector_index = body
        .find("listener_pairing_candidates_from_unpaired_selector(expected_name, direct_addresses)")
        .expect("recovery pairing must pass freshly advertised addresses into Windows selector");

    assert!(
        body.contains("let direct_addresses = if fresh_advertised_addresses.is_empty()"),
        "recovery pairing must use fresh advertised addresses as the selector filter when present"
    );
    assert!(
        scan_index < selector_index,
        "recovery pairing must still pass freshly advertised BLE addresses into Windows selector filtering when direct lookup cannot produce a candidate"
    );
}

#[test]
fn recovery_pairing_limits_direct_candidates_to_fresh_advertisement_when_available() {
    let source = include_str!("embedded_ble.rs");
    let start = source
        .find("fn listener_recovery_pairing_candidates_for_addresses")
        .expect("recovery pairing candidate function should exist");
    let end = source[start..]
        .find("fn listener_recovery_pairing_selector_fallback_candidates")
        .map(|offset| start + offset)
        .expect("recovery pairing selector fallback boundary should exist");
    let body = &source[start..end];

    assert!(body.contains("fresh_advertised_addresses.as_slice()"));
    assert!(body.contains("recovery pairing using fresh advertised address(es)"));
    assert!(
        body.contains(
            "listener_recovery_direct_pairing_candidates(\n            &fresh_advertised_addresses,"
        ),
        "direct pairing must not try stale configured/PnP addresses ahead of the current recovery advertisement"
    );
}

#[test]
fn type_observed_recovery_address_skips_duplicate_pairing_advertisement_scan() {
    let source = include_str!("embedded_ble.rs");
    let helper_start = source
        .find("fn listener_recovery_pairing_candidates_for_addresses")
        .expect("recovery pairing candidate helper should exist");
    let helper_end = source[helper_start..]
        .find("fn listener_recovery_pairing_selector_fallback_candidates")
        .map(|offset| helper_start + offset)
        .expect("recovery pairing helper boundary should exist");
    let helper = &source[helper_start..helper_end];
    let observed_index = helper
        .find("observed_recovery_addresses")
        .expect("Type-observed recovery addresses must enter candidate selection");
    let direct_index = helper
        .find("recovery pairing using Type-observed recovery address(es)")
        .expect("Type-observed addresses should be logged as the first candidate source");
    let scan_index = helper
        .find("scan_listener_pairing_advertisements")
        .expect("ordinary fallback advertisement scan should remain");
    assert!(
        observed_index < direct_index && direct_index < scan_index,
        "once Type has observed the recovery advertisement address, PairAsync must use that address before any duplicate scan"
    );
    assert!(
        helper.contains(
            "let mut addresses = if observed_recovery_addresses.is_empty() {\n        listener_recovery_target_addresses()\n    } else {\n        Vec::new()\n    };"
        ) && helper.contains("listener_recovery_fast_target_addresses()"),
        "a current recovery advertisement address must bypass the redundant PnP address enumeration before direct PairAsync"
    );
    assert!(helper.contains(
        "recovery pairing using {} Type-observed direct address candidate(s) before slow advertisement/AEP discovery"
    ));

    let prompt_start = source
        .find("fn prompt_listener_pairing_inner")
        .expect("pairing prompt helper should exist");
    let prompt_end = source[prompt_start..]
        .find("let mut result = crate::embedded_ble::BleDevicePairingPromptResult")
        .map(|offset| prompt_start + offset)
        .expect("pairing prompt candidate boundary should exist");
    let prompt = &source[prompt_start..prompt_end];
    assert!(
        prompt.contains("if candidate.fresh_pairing_advertisement {\n                continue;\n            }")
            && prompt.contains(
                "trusted_addresses.get_or_insert_with(listener_recovery_target_addresses)"
            ),
        "a freshly advertised candidate must not pay another PnP trust lookup; stale candidates still require the existing trusted-address check"
    );

    let fallback_start = source
        .find("fn listener_recovery_pairing_selector_fallback_candidates_for_addresses")
        .expect("recovery AEP fallback helper should exist");
    let fallback_end = source[fallback_start..]
        .find("fn listener_recovery_direct_pairing_candidates")
        .map(|offset| fallback_start + offset)
        .expect("recovery AEP fallback boundary should exist");
    let fallback = &source[fallback_start..fallback_end];
    assert!(fallback.contains(
        "recovery AEP fallback using Type-observed recovery address(es) without duplicate advertisement scan"
    ));
    assert!(fallback.contains("if fresh_advertised_addresses.is_empty()"));
}

#[test]
fn pairing_only_checks_an_already_paired_known_address_before_scanning() {
    let source = include_str!("embedded_ble.rs");
    let start = source
        .find("fn listener_recovery_pairing_candidates_for_addresses")
        .expect("recovery pairing candidate helper should exist");
    let end = source[start..]
        .find("fn listener_recovery_pairing_selector_fallback_candidates")
        .map(|offset| start + offset)
        .expect("recovery pairing helper boundary should exist");
    let body = &source[start..end];
    let known_index = body
        .find("recovery pairing found {} already-paired known-address candidate(s); skipping the advertisement scan")
        .expect("known paired addresses should have a fast path");
    let scan_index = body
        .find("scan_listener_pairing_advertisements")
        .expect("unpaired recovery must retain advertisement discovery");

    assert!(
        body.contains("if !fast_known_addresses.is_empty()") && body.contains("pairing.IsPaired()"),
        "only a confirmed already-paired known address may bypass recovery advertising"
    );
    assert!(
        known_index < scan_index,
        "pairing-only CLI must not pay the 12-second advertisement scan before returning an already-paired known device"
    );
}

#[test]
fn manual_pairing_only_never_deletes_a_healthy_pair_for_gatt_contention() {
    let source = include_str!("embedded_ble.rs");
    let start = source
        .find("fn prompt_listener_pairing_inner")
        .expect("pairing prompt helper should exist");
    let end = source[start..]
        .find("fn pair_listener_candidates_into_prompt_result")
        .map(|offset| start + offset)
        .expect("pairing prompt helper boundary should exist");
    let body = &source[start..end];

    assert!(body.contains("type_recovery_command_confirmed || !allow_user_pairing_prompt"));
    assert!(
        !body.contains("&target_name,\n        bypass_prompt_suppression,\n        bypass_prompt_suppression,"),
        "the user-facing pairing-only path must not treat a busy GATT status read as proof that an already-paired Windows bond is stale"
    );
}

#[test]
fn already_paired_persisted_address_skips_slow_pnp_trust_enumeration() {
    let source = include_str!("embedded_ble.rs");
    let start = source
        .find("fn pair_listener_candidate(")
        .expect("single pairing candidate helper should exist");
    let end = source[start..]
        .find("fn listener_trusted_paired_candidate_has_fresh_status")
        .map(|offset| start + offset)
        .expect("single pairing candidate helper boundary should exist");
    let body = &source[start..end];
    let fast_trust = body
        .find("let fast_trusted_addresses = listener_recovery_fast_target_addresses()")
        .expect("already-paired candidates should first use in-process trusted addresses");
    let fast_match = body[fast_trust..]
        .find("if !listener_pairing_candidate_has_trusted_address(")
        .map(|offset| fast_trust + offset)
        .expect("fast trusted addresses should validate the candidate");
    let slow_pnp = body[fast_match..]
        .find("let trusted_addresses = listener_recovery_target_addresses()")
        .map(|offset| fast_match + offset)
        .expect("slow PnP trust discovery should remain as a fallback");

    assert!(fast_trust < fast_match && fast_match < slow_pnp);
}

#[test]
fn recovery_pairing_fallback_does_not_use_stale_same_name_cache_when_fresh_address_exists() {
    let source = include_str!("embedded_ble.rs");
    let start = source
        .find("fn listener_recovery_pairing_selector_fallback_candidates")
        .expect("recovery pairing selector fallback should exist");
    let end = source[start..]
        .find("fn listener_recovery_direct_pairing_candidates")
        .map(|offset| start + offset)
        .expect("recovery direct pairing boundary should exist");
    let body = &source[start..end];

    assert!(body.contains("let mut fresh_advertised_addresses = Vec::new();"));
    assert!(body.contains("let selector_addresses = if fresh_advertised_addresses.is_empty()"));
    assert!(body.contains("not falling back to stale same-name cache"));
}

#[test]
fn recovery_pairing_fast_direct_failure_uses_slow_aep_fallback() {
    let source = include_str!("embedded_ble.rs");
    let start = source
        .find("fn prompt_listener_pairing_inner")
        .expect("pairing prompt helper should exist");
    let end = source[start..]
        .find("fn pair_listener_candidates_into_prompt_result")
        .map(|offset| start + offset)
        .expect("pairing prompt result helper should exist");
    let body = &source[start..end];

    assert!(source.contains("BLE_PAIRING_FAST_FAILURE_AEP_FALLBACK_THRESHOLD"));
    assert!(body.contains("fast_recovery_pairing_failure"));
    assert!(body.contains("listener_recovery_pairing_selector_fallback_candidates"));
    assert!(body.contains("failed_before_fallback"));
    assert!(
        body.contains("saturating_sub(failed_before_fallback)"),
        "if slow AEP fallback succeeds after an immediate direct PairAsync failure, the direct failure must not keep the final prompt result failed"
    );
}

#[test]
fn recovery_pairing_fast_aep_fallback_is_bounded() {
    let source = include_str!("embedded_ble.rs");
    let start = source
        .find("fn listener_recovery_pairing_selector_fallback_candidates")
        .expect("recovery pairing selector fallback should exist");
    let end = source[start..]
        .find("fn listener_recovery_direct_pairing_candidates")
        .map(|offset| start + offset)
        .expect("recovery direct pairing boundary should exist");
    let body = &source[start..end];

    assert!(source.contains("BLE_PAIRING_FAST_AEP_FALLBACK_DISCOVERY_TIMEOUT"));
    assert!(
        body.contains("listener_pairing_candidates_from_unpaired_selector_with_timeout")
            && body.contains("BLE_PAIRING_FAST_AEP_FALLBACK_DISCOVERY_TIMEOUT"),
        "rename/Type automatic recovery must not wait the full Windows pairing discovery timeout after direct PairAsync fails"
    );
    assert!(
        source.contains("const BLE_PAIRING_FAST_AEP_FALLBACK_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(6)"),
        "keep the no-user-prompt recovery fallback bounded so full BLE rename recovery does not regress toward 45s"
    );
}

#[test]
fn recovery_pairing_marks_fresh_only_from_current_advertisement_scan() {
    let source = include_str!("embedded_ble.rs");
    let start = source
        .find("fn listener_recovery_pairing_candidates")
        .expect("recovery pairing candidate function should exist");
    let end = source[start..]
        .find("fn listener_recovery_direct_pairing_candidates")
        .map(|offset| start + offset)
        .expect("direct pairing helper should exist");
    let body = &source[start..end];

    assert!(body.contains("let mut fresh_advertised_addresses = Vec::new();"));
    assert!(body.contains("push_unique_address(&mut fresh_advertised_addresses, address);"));
    assert!(body.contains("&fresh_advertised_addresses"));
}

#[test]
fn direct_pairing_does_not_mark_pnp_address_as_fresh_advertisement() {
    let source = include_str!("embedded_ble.rs");
    let start = source
        .find("fn listener_recovery_direct_pairing_candidates")
        .expect("direct pairing helper should exist");
    let end = source[start..]
        .find("fn push_listener_pairing_candidate_if_matching")
        .map(|offset| start + offset)
        .expect("direct pairing helper boundary should exist");
    let body = &source[start..end];

    assert!(body.contains("fresh_advertised_addresses: &[u64]"));
    assert!(
        body.contains("fresh_pairing_advertisement: fresh_advertised_addresses.contains(&address)"),
        "PnP/configured address lookup alone must not be treated as a fresh recovery advertisement"
    );
    assert!(
        !body.contains("fresh_pairing_advertisement: true"),
        "direct address lookup must not blindly allow destructive stale-cache unpair"
    );
}

#[test]
fn embedded_audio_status_probe_uses_bounded_status_path_not_notify_capture() {
    let source = include_str!("embedded_ble.rs");
    let start = source
        .find("pub fn read_embedded_audio_status(\n    timeout: Duration,")
        .expect("Windows status probe entry should exist");
    let end = source[start..]
        .find("pub(super) fn is_transient_notify_target_open_error")
        .map(|offset| start + offset)
        .expect("status probe body boundary should exist");
    let body = &source[start..end];

    assert!(
        body.contains("open_embedded_audio_status_target(timeout)"),
        "status reads must use the bounded status-only target path"
    );
    assert!(
        source.contains("let fresh_status_read = readiness.is_some() || capabilities.is_some();")
            && source.contains("connected: fresh_status_read"),
        "cached Windows service visibility must not be reported as a live BLE link without a fresh status read"
    );
    assert!(
        source.contains("fn read_embedded_audio_status_strings_with_recovery(")
            && source.contains("status readiness recovered on attempt")
            && source.contains("status capabilities recovered on attempt"),
        "status reads must retry transient Windows GATT discovery/read failures within the bounded status timeout"
    );
    assert!(
        !body.contains("open_notify_target()?"),
        "status reads must not open the full notify/capture target because Windows GATT discovery can queue for long periods"
    );
    assert!(source.contains("fn remaining_ble_timeout("));
    assert!(source.contains("fn open_embedded_audio_status_target("));
}

#[test]
fn pairing_discovery_prefers_windows_ble_association_endpoint() {
    let source = include_str!("embedded_ble.rs");
    let start = source
        .find("fn listener_pairing_candidates_from_unpaired_selector")
        .expect("pairing discovery helper should exist");
    let end = source[start..]
        .find("fn listener_pairing_candidates_from_windows_ble_aep")
        .map(|offset| start + offset)
        .expect("AEP pairing helper should exist");
    let body = &source[start..end];
    let aep_index = body
        .find("listener_pairing_candidates_from_windows_ble_aep")
        .expect("pairing discovery must query BLE AEP first");
    let legacy_index = body
        .find("GetDeviceSelectorFromPairingState(false)")
        .expect("legacy BluetoothLEDevice selector should remain as fallback");

    assert!(
        aep_index < legacy_index,
        "Windows BLE pairing discovery must prefer AssociationEndpoint before legacy BLE device interface selector"
    );
    assert!(source.contains("DeviceInformationKind::AssociationEndpoint"));
    assert!(source.contains("WINDOWS_BLE_AEP_SELECTOR"));
}

#[test]
fn pairing_discovery_requires_connectable_aep_before_full_cache() {
    let source = include_str!("embedded_ble.rs");
    let start = source
        .find("fn listener_pairing_candidates_from_windows_ble_aep")
        .expect("AEP pairing helper should exist");
    let end = source[start..]
        .find("fn listener_pairing_candidates_from_windows_ble_aep_selector")
        .map(|offset| start + offset)
        .expect("AEP selector helper boundary should exist");
    let body = &source[start..end];
    let connectable_index = body
        .find("WINDOWS_BLE_AEP_CONNECTABLE_SELECTOR")
        .expect("AEP discovery must query connectable devices first");
    let full_cache_index = body
        .find("WINDOWS_BLE_AEP_SELECTOR")
        .expect("AEP discovery should keep full-cache fallback for diagnostics");

    assert!(
        connectable_index < full_cache_index,
        "Windows BLE pairing must prefer connectable AEPs before stale full-cache AEPs"
    );
    assert!(
        source.contains("WINDOWS_AEP_BLE_IS_CONNECTABLE_PROPERTY")
            || source.contains("WINDOWS_BLE_AEP_CONNECTABLE_SELECTOR")
    );
    assert!(source.contains("connectable_selector={connectable_selector}"));
}

#[test]
fn usb_serial_device_settings_drain_waits_for_quiet_window() {
    let source = include_str!("embedded_ble.rs");
    let exchange_start = source
        .find("fn exchange_device_settings_via_serial_port")
        .expect("device settings serial exchange helper should exist");
    let control_start = source[exchange_start..]
        .find("fn send_control_command_via_serial_port")
        .map(|offset| exchange_start + offset)
        .expect("control serial helper should exist");
    let drain_start = source[control_start..]
        .find("fn drain_serial_input_until_quiet")
        .map(|offset| control_start + offset)
        .expect("quiet drain helper should exist");
    let response_tail_start = source[drain_start..]
        .find("fn response_tail")
        .map(|offset| drain_start + offset)
        .expect("response tail helper should follow quiet drain");
    let production_body = &source[exchange_start..response_tail_start];

    assert!(source.contains("DEVICE_SETTINGS_SERIAL_DRAIN_MAX_DURATION"));
    assert!(source.contains("DEVICE_SETTINGS_SERIAL_DRAIN_QUIET_DURATION"));
    assert!(production_body.contains("fn drain_serial_input_until_quiet"));
    assert!(
        production_body.contains("quiet_since.elapsed() >= quiet_duration"),
        "USB serial fallback must drain stale diagnostic output until a quiet window before sending commands"
    );
    assert!(
        !production_body.contains("drain_serial_input(&mut *port, Duration::from_millis(180))"),
        "USB serial fallback must not use the old fixed 180 ms drain"
    );
}

#[test]
fn fresh_identity_recovery_requires_the_exact_firmware_execution_result() {
    let expected = "VREC:RECOVERY:TYPE:SILENT:FRESH";
    let response = concat!(
        "old diagnostic output\r\n",
        "~VREC:RESULT command=VREC:RECOVERY:TYPE:SILENT result=OK\r\n",
        "~VREC:RESULT command=VREC:RECOVERY:TYPE:SILENT:FRESH result=OK\r\n",
    );
    assert_eq!(
        complete_recovery_control_result(response, expected).as_deref(),
        Some("OK")
    );
    assert_eq!(
        complete_recovery_control_result(
            "~VREC:RESULT command=VREC:RECOVERY:TYPE:SILENT:FRESH result=ESP_FAIL\n",
            expected,
        )
        .as_deref(),
        Some("ESP_FAIL")
    );
    assert!(complete_recovery_control_result(
        "~VREC:RESULT command=VREC:RECOVERY:TYPE:SILENT result=OK\n",
        expected,
    )
    .is_none());

    let source = include_str!("embedded_ble.rs");
    let start = source
        .find("fn send_recovery_control_command_via_serial_port")
        .expect("recovery serial transaction should exist");
    let end = source[start..]
        .find("fn complete_recovery_control_result")
        .map(|offset| start + offset)
        .expect("recovery result parser boundary should exist");
    let transaction = &source[start..end];
    assert!(
        transaction.contains("drain_serial_input_until_quiet")
            && transaction.contains("complete_recovery_control_result(&response, command)")
            && transaction.contains("timed out waiting for exact firmware recovery result")
            && !transaction.contains("std::thread::sleep"),
        "a host serial flush is not recovery confirmation; Type must wait for the exact firmware execution result"
    );
}

#[test]
fn pairing_discovery_rejects_stale_same_name_aep_when_target_address_known() {
    let source = include_str!("embedded_ble.rs");
    let start = source
        .find("fn push_listener_pairing_candidate_if_matching")
        .expect("pairing candidate filter should exist");
    let end = source[start..]
        .find("fn push_listener_pairing_advertisement_candidates")
        .map(|offset| start + offset)
        .expect("pairing candidate filter boundary should exist");
    let body = &source[start..end];
    let stale_guard_index = body
        .find("!target_addresses.is_empty() && address.is_some() && !address_matches")
        .expect("known-address pairing must reject stale same-name Windows cache entries");
    let name_match_index = body
        .find("listener_pairing_name_matches")
        .expect("pairing candidate filter should still support name matching fallback");

    assert!(
        stale_guard_index < name_match_index,
        "known target address must reject stale Windows cache entries before name-only fallback"
    );
    assert!(body.contains("skipping stale Windows pairing candidate"));
}

#[test]
fn already_paired_requires_trusted_address_not_same_name_cache_only() {
    let source = include_str!("embedded_ble.rs");
    let start = source
        .find("fn pair_listener_candidate(\n")
        .expect("pairing helper should exist");
    let end = source[start..]
        .find("fn pair_unpaired_listener_candidate")
        .map(|offset| start + offset)
        .expect("pairing helper boundary should exist");
    let body = &source[start..end];
    let guard_index = body
        .find("listener_pairing_candidate_has_trusted_address")
        .expect("cached AlreadyPaired path must require trusted address evidence");
    let already_index = body
        .find("DevicePairingOutcome::AlreadyPaired")
        .expect("cached AlreadyPaired branch should still exist");

    assert!(
        guard_index < already_index,
        "same-name Windows cache entries must not be reported as already paired unless their address is trusted"
    );
    assert!(
        body.contains("same-name cached pairing without matching Listener address/service proof")
    );
}

#[test]
fn type_recovery_promotes_only_trusted_cached_addresses() {
    let source = include_str!("embedded_ble.rs");
    let start = source
        .find("if type_recovery_command_confirmed")
        .expect("Type recovery promotion block should exist");
    let end = source[start..]
        .find("let mut result = crate::embedded_ble::BleDevicePairingPromptResult")
        .map(|offset| start + offset)
        .expect("Type recovery promotion block boundary should exist");
    let body = &source[start..end];

    let pre_cleanup_index = body
        .find("unpair_listener_devices_inner(std::slice::from_ref(&target_name))")
        .expect("confirmed Type recovery must clean stale Windows pairing before PairAsync");
    let candidate_scan_index = body
        .find("listener_recovery_pairing_candidates_for_addresses")
        .expect("confirmed Type recovery must use recovery-address-aware candidate discovery");
    assert!(
        pre_cleanup_index < candidate_scan_index,
        "confirmed Type recovery must remove stale Windows PnP/bond cache before pairing the Type-observed or freshly scanned recovery address"
    );
    assert!(
        body.contains("get_or_insert_with(listener_recovery_target_addresses)")
            && body.contains("listener_pairing_candidate_has_trusted_address"),
        "confirmed Type recovery may lazily resolve trusted addresses, but every stale cached candidate must still be checked against that trusted set"
    );
    assert!(
        body.contains("same-name cached candidate without trusted address proof"),
        "confirmed Type recovery must not promote arbitrary same-name stale AEP cache entries"
    );
}

#[test]
fn recovery_target_addresses_include_windows_pnp_service_signature() {
    let source = include_str!("embedded_ble.rs");
    let start = source
        .find("fn listener_recovery_target_addresses")
        .expect("recovery target address helper should exist");
    let end = source[start..]
        .find("fn push_listener_recovery_target_addresses_from_candidates")
        .map(|offset| start + offset)
        .expect("recovery target address helper boundary should exist");
    let body = &source[start..end];

    assert!(
        body.contains("configured_bluetooth_address"),
        "recovery address discovery must keep the fast in-process configured address"
    );
    assert!(
        body.contains("listener_pnp_service_signature_addresses"),
        "headless/new Type processes must recover the current Listener address from Windows PnP service UUIDs instead of accepting same-name stale AEP cache entries"
    );
    assert!(source.contains("Listener PnP service-signature addresses"));
}

#[test]
fn windows_pairing_uses_standard_pairasync_by_default() {
    let source = include_str!("embedded_ble.rs");
    let start = source
        .find("fn run_default_pairing_once")
        .expect("default pairing helper should exist");
    let end = source[start..]
        .find("fn pairing_status_should_retry_after_settle")
        .map(|offset| start + offset)
        .expect("default pairing helper boundary should exist");
    let body = &source[start..end];

    assert!(body.contains(".PairAsync()"));
    assert!(
        !body.contains("PairWithProtectionLevelAsync(DevicePairingProtectionLevel::None)"),
        "Windows BLE HID pairing should not force protection level None without hardware evidence"
    );
}

#[test]
fn windows_pairing_uses_custom_first_only_for_exact_type_recovery() {
    let source = include_str!("embedded_ble.rs");
    let start = source
        .find("fn pair_unpaired_listener_candidate")
        .expect("pairing helper should exist");
    let end = source[start..]
        .find("fn run_default_pairing_once")
        .map(|offset| start + offset)
        .expect("pairing helper boundary should exist");
    let body = &source[start..end];
    let custom_first_index = body
        .find("if retry_failed_pairasync_once")
        .expect("fresh Type recovery should have a dedicated first ceremony");
    let custom_index = body[custom_first_index..]
        .find("custom_pair_listener_candidate")
        .map(|offset| custom_first_index + offset)
        .expect("fresh Type recovery should register custom pairing first");
    let standard_index = body
        .find("run_default_pairing_once")
        .expect("pairing helper must attempt standard PairAsync");

    assert!(
        custom_first_index < custom_index && custom_index < standard_index,
        "exact Type-owned fresh recovery must register ConfirmOnly before its first security ceremony"
    );
    assert!(body.contains(
        "if !retry_failed_pairasync_once && pairing_status_should_try_custom_fallback(status)"
    ));
}

#[test]
fn pairing_and_unpair_share_single_maintenance_gate() {
    let source = include_str!("embedded_ble.rs");
    assert!(
        source.contains("LISTENER_PAIRING_MAINTENANCE_TOKEN"),
        "BLE pairing/cache maintenance must have a process-wide owner token"
    );
    assert!(
        source.contains("CreateMutexW")
            && source.contains("WaitForSingleObject")
            && source.contains("BLE_PAIRING_MAINTENANCE_MUTEX_NAME"),
        "GUI tray and headless CLI processes must share a Windows named mutex before touching pairing/cache state"
    );
    assert!(
        source.contains("pub fn listener_pairing_maintenance_active"),
        "coordinator must be able to detect an active pairing/cache owner before cleanup"
    );

    let prompt_start = source
        .find("fn prompt_listener_pairing_inner")
        .expect("pairing prompt helper should exist");
    let prompt_end = source[prompt_start..]
        .find("let mut candidates = if bypass_prompt_suppression")
        .map(|offset| prompt_start + offset)
        .expect("pairing prompt candidate boundary should exist");
    let prompt_setup = &source[prompt_start..prompt_end];
    assert!(
        prompt_setup.contains("try_begin_listener_pairing_maintenance(\"pair\""),
        "PairAsync recovery must acquire the same maintenance gate before touching Windows pairing state"
    );

    let unpair_start = source
        .find("pub fn unpair_listener_devices_for_names")
        .expect("unpair helper should exist");
    let unpair_end = source[unpair_start..]
        .find("fn unpair_listener_devices_inner")
        .map(|offset| unpair_start + offset)
        .expect("unpair helper boundary should exist");
    let unpair_body = &source[unpair_start..unpair_end];
    assert!(
        unpair_body.contains("try_begin_listener_pairing_maintenance(\"unpair\""),
        "Windows cache cleanup must not run while another pairing/cache operation is active"
    );
}

#[test]
fn confirmed_type_recovery_command_allows_direct_stale_cache_cleanup() {
    let source = include_str!("embedded_ble.rs");
    let start = source
        .find("fn prompt_listener_pairing_inner")
        .expect("pairing prompt helper should exist");
    let end = source[start..]
        .find("let mut result = crate::embedded_ble::BleDevicePairingPromptResult")
        .map(|offset| start + offset)
        .expect("pairing prompt candidate boundary should exist");
    let body = &source[start..end];

    assert!(body.contains("type_recovery_command_confirmed"));
    assert!(
        body.contains("candidate.fresh_pairing_advertisement = true"),
        "after Type has successfully commanded Listener into recovery pairing, Windows paired cache for the same address must be treated as stale even if advertisement scanning misses the short window"
    );
    assert!(
        source.contains("pub fn prompt_listener_pairing_after_type_recovery"),
        "the confirmed recovery-command path must stay separate from conservative pairing-only scans"
    );
}

#[test]
fn fresh_recovery_pairing_only_cleanup_keeps_slow_discovery_out_of_the_recovery_budget() {
    let source = include_str!("embedded_ble.rs");
    let direct_cleanup_start = source
        .find("pub fn unpair_listener_pairing_for_known_addresses")
        .expect("pairing-only known-address cleanup helper should exist");
    let direct_cleanup_end = source[direct_cleanup_start..]
        .find("fn unpair_listener_devices_inner")
        .map(|offset| direct_cleanup_start + offset)
        .expect("pairing-only known-address cleanup helper boundary should exist");
    let direct_cleanup = &source[direct_cleanup_start..direct_cleanup_end];
    assert!(
        direct_cleanup.contains(
            "unpair_listener_devices_for_known_addresses_inner(extra_names, addresses, false)"
        ),
        "the fresh direct-pair path must not run PnP/BTHPORT cleanup before PairAsync"
    );

    let exact_cache_start = source
        .find("fn clear_listener_bthport_cache_for_known_addresses_inner")
        .expect("fresh recovery needs an exact-cache cleanup helper");
    let exact_cache_end = source[exact_cache_start..]
        .find("fn finalize_known_address_unpair_result")
        .map(|offset| exact_cache_start + offset)
        .expect("exact-cache cleanup helper boundary should exist");
    let exact_cache = &source[exact_cache_start..exact_cache_end];
    assert!(
        exact_cache.contains("bthport_listener_cache_candidates")
            && exact_cache.contains("true,")
            && !exact_cache.contains("listener_pnp_remove_candidates"),
        "fresh recovery may clean only the exact BTHPORT address after direct PairAsync; it must not enumerate all PnP devices"
    );
    assert!(
        source.contains("fresh recovery-address direct PairAsync failed; skipping slow AEP discovery"),
        "a fresh recovery address must not fall through to slow AEP discovery after direct PairAsync fails"
    );
}

#[test]
fn rename_pairing_failure_preserves_previous_identity_until_type_ready() {
    let source = include_str!("embedded_ble.rs");
    let start = source
        .find("fn recover_listener_pairing_after_type_recovery_for_addresses_inner")
        .expect("atomic Type recovery helper should exist");
    let end = source[start..]
        .find("fn atomic_type_recovery_no_cleanup_result")
        .map(|offset| start + offset)
        .expect("atomic Type recovery helper boundary should exist");
    let body = &source[start..end];
    let direct_pair = body
        .find("let pairing = prompt_listener_pairing_inner")
        .expect("fresh identity PairAsync should exist");
    let preserve_guard = body[direct_pair..]
        .find("if cleanup_addresses.is_empty()")
        .map(|offset| direct_pair + offset)
        .expect("an empty cleanup set must preserve the previous pairing");
    let cache_fallback = body
        .find("clear_listener_bthport_cache_for_known_addresses_inner")
        .expect("non-rename recovery may retain its exact-cache fallback");

    assert!(direct_pair < preserve_guard && preserve_guard < cache_fallback);
    assert!(
        body[preserve_guard..cache_fallback].contains("return Ok((unpair, pairing));")
            && body[preserve_guard..cache_fallback]
                .contains("deferring all previous-identity cleanup until TYPE:READY"),
        "a failed fresh PairAsync in the rename path must return without UnpairAsync, PnP removal, or BTHPORT deletion"
    );
}

#[test]
fn atomic_rename_finishes_exact_predecessor_windows_teardown_before_pairing() {
    let source = include_str!("embedded_ble.rs");
    let start = source
        .find("fn recover_listener_pairing_after_type_recovery_for_addresses_inner")
        .expect("atomic Type recovery helper should exist");
    let end = source[start..]
        .find("fn atomic_type_recovery_no_cleanup_result")
        .map(|offset| start + offset)
        .expect("atomic Type recovery helper boundary should exist");
    let body = &source[start..end];
    let exact_teardown = body
        .find("unpair_listener_devices_for_known_addresses_inner(\n            &cleanup_names,\n            cleanup_addresses,\n            true,")
        .expect("atomic rename must finish exact predecessor PnP/BTHPORT teardown");
    let direct_pair = body
        .find("let pairing = prompt_listener_pairing_inner")
        .expect("fresh identity PairAsync should exist");

    assert!(exact_teardown < direct_pair);
    assert!(!body.contains("prune_listener_ghost_pairings_keeping"));
}

#[test]
fn exact_predecessor_pnp_cleanup_uses_direct_root_without_global_enumeration() {
    let source = include_str!("embedded_ble.rs");
    let start = source
        .find("fn listener_pnp_remove_candidates")
        .expect("PnP cleanup candidate helper should exist");
    let end = source[start..]
        .find("fn push_listener_pnp_entry")
        .map(|offset| start + offset)
        .expect("PnP cleanup candidate helper boundary should exist");
    let body = &source[start..end];
    let exact = body
        .find("if exact_address_only")
        .expect("exact-address PnP branch should exist");
    let global = body
        .find("DeviceInformation::FindAllAsyncDeviceClass(DeviceClass::All)")
        .expect("generic cleanup should retain global PnP enumeration");
    let direct = &body[exact..global];

    assert!(exact < global);
    assert!(direct.contains(r#"format!(r"BTHLE\Dev_{address:012x}")"#));
    assert!(direct.contains("return Ok(candidates);"));
    assert!(!direct.contains("powershell_listener_pnp_entries"));
}

#[test]
fn recent_pairing_notify_uses_only_the_exact_pairasync_address_before_type_ready() {
    let notify_source = std::include_str!("windows_ble/notify_open.rs");
    let start = notify_source
        .find("if let Some(state) = recent_pairing.as_ref()")
        .expect("recent pairing notify fast path should exist");
    let end = notify_source[start..]
        .find("let native_windows_hid_addresses")
        .map(|offset| start + offset)
        .expect("recent pairing notify fast path boundary should exist");
    let fast_path = &notify_source[start..end];
    assert!(
        fast_path.contains("open_notify_target_for_known_addresses_with_cache_modes")
            && fast_path.contains("false,"),
        "a fresh Type-confirmed PairAsync address must not repeat full historical-address/PnP discovery before GATT reopen"
    );

    let source = include_str!("embedded_ble.rs");
    let helper_start = source
        .find("fn open_notify_target_for_known_addresses_with_cache_modes")
        .expect("known-address notify helper should exist");
    let helper_end = source[helper_start..]
        .find("fn recovery_swift_pair_advertisement_visible_for_persisted_address")
        .map(|offset| helper_start + offset)
        .expect("known-address notify helper boundary should exist");
    let helper = &source[helper_start..helper_end];
    assert!(
        helper.contains("if include_recovery_addresses")
            && helper.contains("listener_recovery_target_addresses()"),
        "normal recovery must retain historical-address discovery while the exact recent-pairing path can skip it"
    );
}

#[test]
fn cached_aep_pairing_candidate_never_unpairs_existing_link() {
    let source = include_str!("embedded_ble.rs");
    let start = source
        .find("fn pair_listener_candidate")
        .expect("pair listener helper should exist");
    let end = source[start..]
        .find("fn pair_unpaired_listener_candidate")
        .map(|offset| start + offset)
        .expect("pair listener helper boundary should exist");
    let body = &source[start..end];
    let cache_guard_index = body
        .find("!candidate.fresh_pairing_advertisement")
        .expect("cached/AEP pairing candidates must be guarded");
    let unpair_index = body
        .find("unpair_device_information_pairing")
        .expect("fresh recovery advertisement path may still clean stale pairing");

    assert!(
        cache_guard_index < unpair_index,
        "Windows cached/AEP candidates must not unpair an existing Listener link; only fresh recovery advertisements can trigger stale-cache cleanup"
    );
    assert!(body.contains("AlreadyPaired"));
    assert!(body.contains("leaving pairing intact"));
}

#[test]
fn recovery_already_paired_cache_requires_fresh_gatt_before_success() {
    let source = include_str!("embedded_ble.rs");
    let start = source
        .find("fn pair_listener_candidate")
        .expect("pair listener helper should exist");
    let end = source[start..]
        .find("fn pair_unpaired_listener_candidate")
        .map(|offset| start + offset)
        .expect("pair listener helper boundary should exist");
    let body = &source[start..end];
    let verify_index = body
        .find("verify_already_paired_liveness")
        .expect("recovery path must have an already-paired liveness gate");
    let fresh_status_index = body
        .find("listener_trusted_paired_candidate_has_fresh_status")
        .expect("already-paired recovery must prove fresh GATT status before success");
    let stale_cleanup_index = body
        .find("fresh GATT status is not live during recovery; clearing stale host bond before PairAsync")
        .expect("stale host bond cleanup log should name the Windows cache failure mode");
    let force_unpair_index = body
        .find("candidate.fresh_pairing_advertisement = true")
        .expect("failed liveness must force the existing stale cleanup path");
    let unpair_index = body
        .find("unpair_device_information_pairing")
        .expect("recovery stale cache path must still unpair Windows first");

    assert!(verify_index < fresh_status_index);
    assert!(fresh_status_index < stale_cleanup_index);
    assert!(stale_cleanup_index < force_unpair_index);
    assert!(force_unpair_index < unpair_index);

    let helper_start = source
        .find("fn listener_trusted_paired_candidate_has_fresh_status")
        .expect("fresh GATT liveness helper should exist");
    let helper_end = source[helper_start..]
        .find("fn pair_unpaired_listener_candidate")
        .map(|offset| helper_start + offset)
        .expect("fresh GATT liveness helper boundary should exist");
    let helper = &source[helper_start..helper_end];
    assert!(helper.contains("open_embedded_audio_status_target_for_device"));
    assert!(helper.contains("read_embedded_audio_status_from_target_bounded"));
    assert!(helper.contains("status.connected"));
}

#[test]
fn stale_pairing_refresh_uses_known_address_before_selector_query() {
    let source = include_str!("embedded_ble.rs");
    let start = source
        .find("fn refresh_listener_pairing_candidate")
        .expect("refresh helper should exist");
    let end = source[start..]
        .find("fn unpair_listener_candidate")
        .map(|offset| start + offset)
        .expect("refresh helper boundary should exist");
    let body = &source[start..end];
    let direct_index = body
        .find("pairing_device_information_from_bluetooth_address_handle")
        .expect("refresh should use direct Bluetooth address handle open");
    let selector_index = body
        .find("listener_pairing_candidates")
        .expect("refresh should still keep selector fallback");

    assert!(
        direct_index < selector_index,
        "after stale unpair, a known Listener address should be reopened directly before waiting on the slower Windows AEP selector"
    );
}

#[test]
fn desktop_pairasync_failed_uses_custom_pairing_fallback() {
    let source = include_str!("embedded_ble.rs");
    let start = source
        .find("fn pairing_status_should_try_custom_fallback")
        .expect("custom fallback status helper should exist");
    let end = source[start..]
        .find("fn custom_pair_listener_candidate")
        .map(|offset| start + offset)
        .expect("custom pairing helper boundary should exist");
    let body = &source[start..end];

    assert!(
        body.contains("DevicePairingResultStatus::Failed"),
        "desktop Windows PairAsync can return Failed(19) before the system dialog, so Type must try custom ConfirmOnly pairing"
    );
}

#[test]
fn pairasync_failure_does_not_restart_windows_bluetooth_adapter_automatically() {
    let source = include_str!("embedded_ble.rs");
    let prompt_start = source
        .find("fn prompt_listener_pairing_inner")
        .expect("pairing prompt helper should exist");
    let prompt_end = source[prompt_start..]
        .find("fn pair_listener_candidates_into_prompt_result")
        .map(|offset| prompt_start + offset)
        .expect("pairing prompt helper boundary should exist");
    let prompt = &source[prompt_start..prompt_end];
    assert!(
        prompt.contains("let allow_adapter_restart = false;"),
        "Type PairAsync must not tear down a concurrent native Windows pairing ceremony"
    );

    let start = source
        .find("fn pair_unpaired_listener_candidate_with_adapter_recovery")
        .expect("adapter recovery pairing helper should exist");
    let end = source[start..]
        .find("fn run_default_pairing_once")
        .map(|offset| start + offset)
        .expect("adapter recovery pairing helper boundary should exist");
    let body = &source[start..end];

    assert!(body.contains("pairing_status_suggests_adapter_restart(status)"));
    assert!(body.contains("&& allow_adapter_restart"));
    assert!(body.contains("restart_windows_bluetooth_adapter_after_pairing_failure"));
    assert!(
        body.contains("refresh_listener_pairing_candidate(candidate, expected_name)"),
        "the dormant explicit adapter-recovery branch must remain bounded"
    );
    assert!(
        body.contains("!allow_adapter_restart")
            && body.contains("passive in-progress PairAsync settle"),
        "Type automatic recovery should wait briefly for Windows in-progress pairing instead of bouncing the local Bluetooth adapter"
    );
    assert!(
        body.contains("false,"),
        "the dormant adapter-recovery retry must still disable another restart"
    );
    assert!(
        source.contains("fn pairing_status_suggests_adapter_restart")
            && source.contains("DevicePairingResultStatus::Failed"),
        "the dormant diagnostic branch must remain restricted to Failed(19)"
    );
}

#[test]
fn type_recovery_failed_pairasync_retries_once_without_adapter_restart() {
    let source = include_str!("embedded_ble.rs");
    let prompt_start = source
        .find("fn prompt_listener_pairing_inner")
        .expect("prompt helper should exist");
    let prompt_end = source[prompt_start..]
        .find("fn pairing_prompt_suppression_remaining")
        .map(|offset| prompt_start + offset)
        .expect("prompt helper boundary should exist");
    let prompt_body = &source[prompt_start..prompt_end];
    assert!(prompt_body.contains("type_recovery_command_confirmed"));
    assert!(
        prompt_body.contains("retry_failed_pairasync_once")
            && prompt_body.contains("type_recovery_command_confirmed,"),
        "Type-confirmed recovery should enable the bounded one-shot PairAsync retry"
    );
    let fallback_start = prompt_body
        .find("fallback_candidates,")
        .expect("slow AEP fallback invocation should exist");
    let fallback_end = prompt_body[fallback_start..]
        .find(");")
        .map(|offset| fallback_start + offset)
        .expect("slow AEP fallback invocation should close");
    let fallback_args: String = prompt_body[fallback_start..fallback_end]
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    assert!(
        fallback_args
            .contains("fallback_candidates,&target_name,false,true,allow_adapter_restart,false,"),
        "slow AEP fallback should not recursively enable the direct PairAsync retry"
    );

    let retry_start = source
        .find("fn pair_unpaired_listener_candidate_with_adapter_recovery")
        .expect("pairing helper should exist");
    let retry_end = source[retry_start..]
        .find("fn run_default_pairing_once")
        .map(|offset| retry_start + offset)
        .expect("pairing helper boundary should exist");
    let retry_body = &source[retry_start..retry_end];
    assert!(
        retry_body.contains("&& retry_failed_pairasync_once")
            && retry_body.contains("&& !allow_adapter_restart")
            && retry_body.contains("pairing_status_suggests_adapter_restart(status)"),
        "the retry may only fire for Type recovery Failed(19) when local adapter restart is disabled"
    );
    assert!(
        retry_body.contains("&& !retry_failed_pairasync_once\n        && !allow_adapter_restart"),
        "when Type recovery has an explicit one-shot Failed(19) retry, it must not wait through the passive in-progress settle first"
    );
    let retry_index = retry_body
        .find("Type automatic recovery retrying direct PairAsync once after Windows Failed(19)")
        .expect("Type recovery retry log should exist");
    let adapter_restart_index = retry_body
        .find("restart_windows_bluetooth_adapter_after_pairing_failure")
        .expect("adapter restart branch should still exist");
    assert!(
        retry_index < adapter_restart_index,
        "Type recovery should retry the fresh direct PairAsync path before any adapter-restart branch"
    );
    assert!(
        retry_body.contains("&refreshed_pairing,\n                    expected_name,\n                    false,\n                    false,"),
        "the retry must disable both adapter restart and another retry to avoid a loop"
    );
}

#[test]
fn type_recovery_pairasync_disables_adapter_restart() {
    let source = include_str!("embedded_ble.rs");
    let start = source
        .find("fn prompt_listener_pairing_inner")
        .expect("prompt helper should exist");
    let end = source[start..]
        .find("fn pairing_prompt_suppression_remaining")
        .map(|offset| start + offset)
        .expect("prompt helper boundary should exist");
    let body = &source[start..end];

    assert!(
        body.contains("let allow_adapter_restart = false;"),
        "Type-owned pairing must never restart the whole Windows Bluetooth adapter"
    );
    assert!(
        body.contains("allow_adapter_restart,"),
        "the Type recovery adapter-restart policy must flow into every pairing candidate attempt"
    );
}

#[test]
fn rename_recovery_can_skip_duplicate_pre_pair_cleanup_after_cache_refresh() {
    let source = include_str!("embedded_ble.rs");
    // Prefer the Windows impl that actually calls prompt_listener_pairing_inner
    // (mod.rs only has thin wrappers).
    let call_marker =
        "prompt_listener_pairing_inner(expected_name, true, true, false, true, &[], false)";
    let call_at = source
        .find(call_marker)
        .expect("Type recovery prompt helper should exist");
    let start = source[..call_at]
        .rfind("pub fn prompt_listener_pairing_after_type_recovery_without_user_prompt")
        .expect("Type recovery prompt helper should exist");
    let end = source[start..]
        .find("pub fn query_listener_pairing")
        .map(|offset| start + offset)
        .expect("Type recovery prompt helper boundary should exist");
    let helpers = &source[start..end];
    let prompt_start = source
        .find("fn prompt_listener_pairing_inner")
        .expect("prompt helper should exist");
    let prompt_end = source[prompt_start..]
        .find("let mut candidates = if bypass_prompt_suppression")
        .map(|offset| prompt_start + offset)
        .expect("prompt helper cleanup boundary should exist");
    let prompt_setup = &source[prompt_start..prompt_end];

    assert!(helpers.contains(
        "prompt_listener_pairing_after_type_recovery_without_user_prompt_after_cache_cleanup"
    ));
    assert!(helpers.contains(call_marker));
    assert!(helpers.contains(
        "prompt_listener_pairing_inner(\n        expected_name,\n        true,\n        true,\n        false,\n        false,\n        observed_recovery_addresses,"
    ));
    assert!(
        prompt_setup.contains("type_recovery_command_confirmed && pre_pair_stale_cleanup"),
        "double-click/one-click Type recovery must keep pre-pair stale cleanup, while BLE rename uses its explicit empty-cleanup atomic handoff"
    );
}

#[test]
fn windows_pairing_recovery_uses_hidden_pwsh_not_windows_powershell() {
    let source = include_str!("embedded_ble.rs");
    let production = source.as_str(); // path-separated tests: include_str production only

    assert!(
        !production.contains("powershell.exe"),
        "workflow/product diagnostics must not spawn Windows PowerShell 5.1 or visible pwsh windows"
    );
    let command = denzic_ble_windows::hidden_pwsh_command();
    assert_eq!(command.get_program().to_string_lossy(), "pwsh");
}

#[test]
fn custom_pairing_keeps_v1_confirm_ceremonies() {
    let source = include_str!("embedded_ble.rs");
    let start = source
        .find("fn custom_pair_listener_candidate")
        .expect("custom pairing helper should exist");
    let end = source[start..]
        .find("fn pairing_kind_accepts_without_user")
        .map(|offset| start + offset)
        .expect("custom pairing helper boundary should exist");
    let body = &source[start..end];

    assert!(body.contains("DevicePairingKinds::ConfirmOnly"));
    assert!(
        body.contains("DevicePairingKinds::ConfirmPinMatch"),
        "Windows may request the SC confirm ceremony; keep the accepted v1.0.1 custom pairing kinds"
    );
    assert!(body.contains(".PairAsync(supported_pairing_kinds)"));
    assert!(
        !body.contains("DevicePairingProtectionLevel::None"),
        "do not silently request unprotected HID keyboard pairing"
    );
}

#[test]
fn pairasync_failure_does_not_request_native_windows_pairing_window() {
    let source = include_str!("embedded_ble.rs");
    let production = source.as_str(); // path-separated tests: include_str production only
    let start = source
        .find("fn prompt_listener_pairing_inner")
        .expect("pairing prompt helper should exist");
    let end = source[start..]
        .find("fn pairing_prompt_suppression_remaining")
        .map(|offset| start + offset)
        .expect("pairing prompt helper boundary should exist");
    let body = &source[start..end];

    assert!(body.contains("result.failed_devices > 0"));
    assert!(
        !body.contains("send_recording_control_native_pairing_recovery"),
        "failed Windows PairAsync during Type recovery must not request an extra Windows native pairing toast"
    );
    assert!(!production.contains("LAST_NATIVE_PAIRING_WINDOW"));
    assert!(!production.contains("BLE_NATIVE_PAIRING_WINDOW_SUPPRESS"));
}

#[test]
fn pairing_prompt_syncs_expected_name_before_control_fallback() {
    let source = include_str!("embedded_ble.rs");
    let start = source
        .find("fn prompt_listener_pairing_inner")
        .expect("pairing prompt helper should exist");
    let end = source[start..]
        .find("let now = Instant::now();")
        .map(|offset| start + offset)
        .expect("pairing prompt setup boundary should exist");
    let body = &source[start..end];

    assert!(body.contains("let target_name = effective_bluetooth_target_name"));
    assert!(
        body.contains("set_configured_bluetooth_target_name(&target_name);"),
        "expected BLE name must be applied before GATT control fallback scans advertisements"
    );
}

#[test]
fn type_controlled_recovery_uses_explicit_type_commands() {
    let source = include_str!("embedded_ble.rs");
    let production = source.as_str(); // path-separated tests: include_str production only
                                      // Prefer the Windows impl body (mod.rs only has a thin wrapper).
    let marker = "b\"VREC:RECOVERY:TYPE\\n\"";
    let marker_at = source
        .find(marker)
        .expect("type recovery command should exist");
    let start = source[..marker_at]
        .rfind("pub fn send_recording_control_recovery")
        .expect("type recovery command should exist");
    let end = source[start..]
        .find("pub fn send_recording_processing_state")
        .map(|offset| start + offset)
        .or_else(|| source[start..].find("\n}\n\n").map(|o| start + o + 2))
        .expect("type recovery boundary should exist");
    let body = &source[start..end];

    assert!(body.contains("\"VREC:RECOVERY:TYPE\""));
    assert!(body.contains(marker));
    assert!(
        !body.contains("b\"VREC:RECOVERY\\n\""),
        "normal Type-controlled recovery should use the explicit Swift-Pair-capable firmware command"
    );
    assert!(source.contains("pub fn send_recording_control_silent_recovery"));
    assert!(source.contains("\"VREC:RECOVERY:TYPE:SILENT\""));
    assert!(source.contains("b\"VREC:RECOVERY:TYPE:SILENT\\n\""));
    assert!(source.contains("\"VREC:RECOVERY:TYPE:SILENT:FRESH\""));
    assert!(source.contains("b\"VREC:RECOVERY:TYPE:SILENT:FRESH\\n\""));
    assert!(source.contains("pub fn send_recording_control_manual_pairing"));
    assert!(source.contains("\"VREC:RECOVERY:TYPE:MANUAL\""));
    assert!(source.contains("b\"VREC:RECOVERY:TYPE:MANUAL\\n\""));
    assert!(!production.contains("send_recording_control_native_pairing_recovery"));
}

#[test]
fn type_bye_is_a_separate_shutdown_heartbeat_command() {
    let source = include_str!("embedded_ble.rs");
    // Prefer the Windows impl body (mod.rs only has a thin wrapper).
    let start = source
        .find("send_audio_control_via_active_capture(b\"TYPE:BYE\\n\"")
        .expect("Type bye command should exist");
    let body_start = source[..start]
        .rfind("pub fn send_recording_control_type_bye")
        .expect("Type bye command should exist");
    let end = source[body_start..]
        .find("pub fn send_recording_processing_state")
        .map(|offset| body_start + offset)
        .or_else(|| {
            source[body_start..]
                .find("\n}\n\n")
                .map(|o| body_start + o + 2)
        })
        .expect("Type bye command boundary should exist");
    let body = &source[body_start..end];

    assert!(body.contains("b\"TYPE:BYE\\n\""));
    assert!(body.contains("send_audio_control_via_active_capture"));
    assert!(
        !body.contains("send_recording_control_command"),
        "Type shutdown bye must not open a fresh GATT control target; tray quit should not run the long reconnect retry chain"
    );
    assert!(
        !body.contains("VREC:RECOVERY"),
        "Type shutdown must only clear Type-ready heartbeat, not open pairing recovery"
    );
}

#[test]
fn audio_control_advertisement_fallback_requires_windows_pairing() {
    let source = include_str!("embedded_ble.rs");
    let production = source.as_str(); // path-separated tests: include_str production only
    assert!(!production.contains("TryFreshGattAllowUnpairedAdvertisement"));
    assert!(!production.contains("allowing unpaired audio control advertisement GATT fallback"));
    assert!(
        production.contains("ensure_paired_listener_for_advertisement_gatt(\"audio control\""),
        "audio control advertisement fallback must require Windows pairing"
    );
}

#[test]
fn advertisement_gatt_pairing_check_accepts_pnp_service_signature_cache_only_after_pairasync() {
    let source = include_str!("embedded_ble.rs");
    let start = source
        .find("fn paired_listener_device_visible_for_addresses")
        .expect("advertisement pairing visibility helper should exist");
    let end = source[start..]
        .find("fn open_notify_target_for_known_addresses")
        .map(|offset| start + offset)
        .expect("advertisement pairing visibility helper boundary should exist");
    let body = &source[start..end];

    assert!(
        body.contains("allow_pnp_service_signature_cache"),
        "PnP service-signature cache is only pairing evidence after Type has just completed PairAsync"
    );
    assert!(
        body.contains("listener_pnp_service_signature_addresses()"),
        "recent PairAsync notify recovery may accept Windows PnP Listener service nodes while AEP pairing cache lags behind"
    );
    assert!(
        body.contains("addresses.contains(address)"),
        "PnP fallback must still match the advertised Bluetooth address"
    );
    assert!(
        body.contains("allowing advertised GATT fallback while AEP pairing cache refreshes"),
        "runtime logs should distinguish PnP-backed pairing evidence from unpaired advertisement access"
    );

    assert!(source.contains(
        "ensure_paired_listener_for_advertisement_gatt(\"audio notify\", &addresses, false)"
    ));
    // Audio control (TYPE:OTA handoff) allows PnP signature while AEP pairing
    // cache lags after re-pair/full flash — same evidence model as recent PairAsync notify.
    assert!(source.contains(
        "ensure_paired_listener_for_advertisement_gatt(\"audio control\", &addresses, true)"
    ));
    assert!(source.contains(
        "ensure_paired_listener_for_advertisement_gatt(\"recent pairing audio notify\", &addresses, true)"
    ));
}

#[test]
fn post_confirm_ota_notify_reopen_uses_bounded_retry_profile() {
    assert_eq!(
        notify_target_open_retry_delays(true),
        &[Duration::from_millis(250), Duration::from_millis(500)]
    );
    assert_eq!(notify_target_open_retry_delays(false).len(), 7);
}

#[test]
fn post_confirm_ota_notify_reopen_uses_only_the_verified_current_address() {
    let source = include_str!("embedded_ble.rs");
    let retry_start = source
        .find("fn open_notify_target_with_retry")
        .expect("notify retry helper should exist");
    let retry_end = source[retry_start..]
        .find("fn notify_target_open_retry_delays")
        .map(|offset| retry_start + offset)
        .expect("notify retry helper boundary should exist");
    let retry_body = &source[retry_start..retry_end];

    assert!(retry_body.contains("take_listener_ota_post_confirm_notify_target_address()"));
    assert!(retry_body.contains(
        "Some(address) => open_notify_target_for_post_confirm_native_windows_hid(address)"
    ));
    assert!(retry_body.contains("None => open_notify_target()"));
}

#[test]
fn post_confirm_ota_notify_reuses_challenge_verified_cached_gatt() {
    let source = include_str!("embedded_ble.rs");
    let start = source
        .find("fn open_notify_target_for_post_confirm_native_windows_hid(")
        .expect("post-confirm native-HID opener should exist");
    let end = source[start..]
        .find("fn open_notify_target_for_device_with_cache_modes(")
        .map(|offset| start + offset)
        .expect("post-confirm native-HID opener boundary should exist");
    let body = &source[start..end];

    assert!(body.contains("&POST_OTA_VERIFIED_CACHED_CACHE_MODES"));
    assert!(body.contains("POST_OTA_VERIFIED_CACHED_GATT_TIMEOUT"));
    assert!(!body.contains("&POST_OTA_UNCACHED_CACHE_MODES"));
    assert!(body.contains("require_audio_control_for_notify_target"));
}

#[test]
fn gatt_cache_policy_adapter_maps_platform_table_to_winrt_modes() {
    use denzic_ble_pairing::GattCachePolicy;

    assert_eq!(
        bluetooth_cache_modes_for_policy(GattCachePolicy::CachedOnly),
        &[BluetoothCacheMode::Cached]
    );
    assert_eq!(
        bluetooth_cache_modes_for_policy(GattCachePolicy::UncachedOnly),
        &[BluetoothCacheMode::Uncached]
    );
    assert_eq!(
        bluetooth_cache_modes_for_policy(GattCachePolicy::UncachedFirst),
        &[BluetoothCacheMode::Uncached, BluetoothCacheMode::Cached]
    );
    assert_eq!(
        bluetooth_cache_modes_for_policy(GattCachePolicy::CachedFirst),
        &[BluetoothCacheMode::Cached, BluetoothCacheMode::Uncached]
    );
}

#[test]
fn persisted_notify_fast_path_validates_gatt_before_recovery_advertising() {
    let source = include_str!("embedded_ble.rs");
    let open_start = source
        .find("fn open_notify_target()")
        .expect("notify target helper should exist");
    let open_end = source[open_start..]
        .find("fn open_notify_target_with_retry")
        .map(|offset| open_start + offset)
        .expect("notify target helper boundary should exist");
    let open_body = &source[open_start..open_end];
    let persisted_branch_start = open_body
        .find("if recent_pairing.is_none()")
        .expect("persisted startup notify path should follow recent PairAsync recovery");
    let persisted_body = &open_body[persisted_branch_start..];
    let persisted_index = persisted_body
        .find("open_notify_target_for_startup_cached_address(address,")
        .expect("persisted startup notify path should still exist");
    let selector_index = persisted_body
        .find("let selector = GattDeviceService::GetDeviceSelectorFromUuid")
        .expect("normal service discovery should follow the persisted startup path");
    assert!(
        persisted_index < selector_index,
        "the persisted bond/GATT path must run before slower discovery and recovery"
    );
    assert!(
        !persisted_body[..selector_index]
            .contains("recovery_swift_pair_advertisement_visible_for_persisted_address"),
        "cached Swift Pair metadata must not preempt a working persisted bond; recovery advertising is evidence only after a real GATT/CCCD failure"
    );

    let scan_start = source
        .find("fn listener_swift_pair_advertisement_visible_for_address")
        .expect("Swift Pair guard scan helper should exist");
    let scan_end = source[scan_start..]
        .find("fn diagnostic_target_candidates")
        .map(|offset| scan_start + offset)
        .expect("Swift Pair guard scan helper boundary should exist");
    let scan_body = &source[scan_start..scan_end];
    assert!(scan_body.contains("advertisement_swift_pair_display_name"));
    assert!(
        scan_body.contains("address != target_address"),
        "native/random-address pairing windows must not be mistaken for the persisted Type-owned address"
    );

    let capture_start = source
        .find("fn capture_notification_events_until_cancelled_impl")
        .expect("notify capture helper should exist");
    // After include! split, capture lives in capture_events.rs and later siblings are
    // concatenated after it (type_heartbeat stays earlier in windows_ble/mod.rs).
    let capture_end = source[capture_start..]
        .find("fn open_notify_target_for_startup_cached_address")
        .map(|offset| capture_start + offset)
        .expect("notify capture helper boundary should exist");
    let capture_body = &source[capture_start..capture_end];
    let reset_index = capture_body
        .find("notify CCCD reset hit recovery pairing window")
        .expect("CCCD reset cancellation should enter Type recovery");
    let enable_index = capture_body
        .find("enabling notify CCCD")
        .expect("notify enable log should remain after reset handling");
    assert!(
        reset_index < enable_index,
        "reset-time recovery-window cancellation must not burn the notify-enable retry ladder before PairAsync"
    );

    let cccd_start = source
        .find("fn write_cccd_notify_with_retry")
        .expect("notify CCCD helper should exist");
    let cccd_end = source[cccd_start..]
        .find("fn write_cccd_indicate_with_retry")
        .map(|offset| cccd_start + offset)
        .expect("notify CCCD helper boundary should exist");
    let cccd_body = &source[cccd_start..cccd_end];
    assert!(cccd_body.contains("recovery_probe_address"));
    assert!(cccd_body.contains("cccd_notify_recovery_pairing_error"));
    assert!(cccd_body.contains("0x800704C7"));
    assert!(
        cccd_body.contains("denzic_ble_windows::write_cccd_with_retry"),
        "recovery-window CCCD cancellation must delegate the retry ladder to the shared platform helper so PairAsync recovery can preempt it"
    );

    assert!(source.contains("\"diagnostic log\", &data, CCCD_ENABLE_TIMEOUT, None"));
    assert!(source.contains("cleanup.target.bluetooth_address"));
}

#[test]
fn post_confirm_ota_reuses_the_windows_restored_cccd() {
    let source = include_str!("embedded_ble.rs");

    assert!(source.contains("target.post_ota_preserved_cccd = ota_post_confirm_address.is_some();"));
    assert!(source.contains("reusing Windows-restored notify CCCD after verified OTA reconnect"));
    assert!(source.contains("if cleanup.target.post_ota_preserved_cccd"));
}

#[test]
fn recent_pairing_notify_recovery_uses_fresh_advertisement_not_stale_configured_address() {
    let source = include_str!("embedded_ble.rs");
    let notify_start = source
        .find("fn open_notify_target()")
        .expect("notify target helper should exist");
    let notify_end = source[notify_start..]
        .find("fn open_notify_target_with_retry")
        .map(|offset| notify_start + offset)
        .expect("notify retry helper boundary should exist");
    let notify_body = &source[notify_start..notify_end];
    let helper_start = source
        .find("fn open_notify_target_from_recent_pairing_advertisement")
        .expect("recent pairing advertisement helper should exist");
    let helper_end = source[helper_start..]
        .find("fn open_audio_control_target_from_advertisement")
        .map(|offset| helper_start + offset)
        .expect("recent pairing advertisement helper boundary should exist");
    let helper_body = &source[helper_start..helper_end];

    assert!(
        notify_body.contains("open_notify_target_from_recent_pairing_advertisement(state)"),
        "after PairAsync succeeds, notify recovery must try the fresh paired advertisement path instead of waiting on Windows service-index cache"
    );
    assert!(
        !notify_body.contains("skipping 12s advertisement fallback"),
        "recent pairing must not block advertisement recovery while Windows refreshes the service table"
    );
    assert!(helper_body.contains("scan_ble_advertisements_by_name"));
    assert!(helper_body.contains("&state.target_name"));
    assert!(helper_body.contains("ensure_paired_listener_for_advertisement_gatt("));
    assert!(helper_body.contains("\"recent pairing audio notify\""));
    assert!(helper_body.contains("&addresses"));
    assert!(helper_body.contains("true"));
    assert!(
        !helper_body.contains("audio_target_advertisement_addresses"),
        "recent pairing fallback must not reuse stale configured Bluetooth addresses"
    );
}

#[test]
fn persisted_startup_notify_address_is_named_bounded_and_after_recent_pairing() {
    let source = include_str!("embedded_ble.rs");
    let notify_start = source
        .find("fn open_notify_target()")
        .expect("notify target helper should exist");
    let notify_end = source[notify_start..]
        .find("fn open_notify_target_with_retry")
        .map(|offset| notify_start + offset)
        .expect("notify retry helper boundary should exist");
    let notify_body = &source[notify_start..notify_end];
    let recent_index = notify_body
        .find("recent_pairing_fast_gatt_active")
        .expect("recent pairing path must remain first");
    let runtime_index = notify_body
        .find("runtime_bluetooth_target_address()")
        .expect("same-process verified address fast path should exist");
    let native_hid_index = notify_body
        .find("native_windows_hid_pairing_addresses_for_startup")
        .expect("a current native HID address must be available before stale caches");
    let persisted_index = notify_body
        .find("persisted_successful_notify_target_address_for_current")
        .expect("startup persisted address fast path should exist");
    let service_selector_index = notify_body
        .find("GetDeviceSelectorFromUuid(SERVICE_UUID)")
        .expect("service selector fallback should remain");

    assert!(
        recent_index < native_hid_index
            && native_hid_index < runtime_index
            && runtime_index < persisted_index
            && persisted_index < service_selector_index,
        "recent-pairing recovery stays first; a current native HID identity must win over runtime and persisted cache addresses before service discovery"
    );
    assert!(
        notify_body.contains("prioritize_native_hid_notify_addresses"),
        "multi-root HID ghosts must prefer last-successful/runtime identities before timing out dead addresses"
    );
    assert!(notify_body.contains("if recent_pairing.is_none()"));
    assert!(notify_body.contains("if native_windows_hid_addresses.is_empty()"));
    assert!(source.contains(
        "const STARTUP_NOTIFY_FAST_PATH_TIMEOUT: Duration = Duration::from_millis(2500)"
    ));
    assert!(source.contains("open_notify_target_for_startup_cached_address"));
    assert!(source.contains("BluetoothCacheMode::Cached"));
    assert!(source.contains("open_write_characteristic_from_service_with_timeout"));
    assert!(
        source.contains("AUDIO_CONTROL_UUID")
            && source.contains(
                "persist_successful_notify_target_address(address, \"Type heartbeat ready\")"
            ),
        "startup fast path must keep audio control and persist only after Type heartbeat ready"
    );
}

#[test]
fn native_windows_hid_takeover_stays_on_current_identity_without_advertisement_wait() {
    let source = include_str!("embedded_ble.rs");
    let notify_start = source
        .find("fn open_notify_target()")
        .expect("notify target helper should exist");
    let notify_end = source[notify_start..]
        .find("fn open_notify_target_with_retry")
        .map(|offset| notify_start + offset)
        .expect("notify target helper boundary should exist");
    let notify_body = &source[notify_start..notify_end];
    let native_pairing_index = notify_body
        .find("native_windows_hid_pairing_visible_for_startup")
        .expect("native Windows HID pairing marker should exist");
    let service_selector_index = notify_body
        .find("GetDeviceSelectorFromUuid(SERVICE_UUID)")
        .expect("system GATT service selector should remain");

    assert!(
        native_pairing_index < service_selector_index,
        "native Windows HID recovery must be considered before the general system service selector"
    );
    assert!(notify_body
        .contains("selected native Windows HID startup audio notify address={address:012X}"));
    assert!(
        notify_body.contains("for address in &native_windows_hid_addresses")
            && notify_body.contains("open_notify_target_for_current_native_windows_hid(*address)")
            && notify_body.contains("native_windows_hid_snapshot_refresh_is_useful")
            && notify_body.contains(
                "open_notify_target_for_current_native_windows_hid_service_endpoint("
            ),
        "a native HID identity must retry only its exact current service endpoint after the direct uncached path misses"
    );
    assert!(source.contains("open_notify_target_for_device_with_cache_modes_and_timeout("));
    assert!(source.contains("STARTUP_NATIVE_HID_PERSISTED_GATT_TIMEOUT"));
    let native_service_start = source
        .find("fn open_notify_target_for_current_native_windows_hid_service_endpoint(")
        .expect("current native HID service endpoint helper should exist");
    let native_service_end = source[native_service_start..]
        .find("fn open_notify_target()")
        .map(|offset| native_service_start + offset)
        .expect("current native HID service endpoint helper boundary should exist");
    let native_service_body = &source[native_service_start..native_service_end];
    assert!(
        native_service_body.contains("addresses.contains(&address)")
            && native_service_body.contains(
                "open_notify_target_for_service_with_timeout(&id, timeout)"
            )
            && !native_service_body.contains("ble_candidate_allowed"),
        "native HID service fallback must remain an exact-address endpoint open without name-based candidate selection"
    );
    assert!(
        notify_body.contains("skipping stale service-selector address")
            && notify_body.contains("native_windows_hid_addresses.contains(&address)"),
        "after a fresh native HID identity is known, service-selector fallback must not retry a stale Windows BLE identity before retrying the current device"
    );
    let legacy_advertisement_helper = format!(
        "{}{}",
        "open_notify_target_from_native_windows_hid_", "advertisement"
    );
    assert!(
        !source.contains(&legacy_advertisement_helper),
        "native HID pairing can be connected but no longer discoverable by advertisement, so takeover must not wait for an advertisement"
    );
}

#[test]
fn stale_native_windows_hid_snapshot_refreshes_only_current_direct_identities() {
    assert!(native_windows_hid_snapshot_refresh_is_useful(
        &[0x1111],
        &[0x2222]
    ));
    assert!(!native_windows_hid_snapshot_refresh_is_useful(
        &[0x1111],
        &[0x1111]
    ));
    assert!(!native_windows_hid_snapshot_refresh_is_useful(
        &[0x1111],
        &[]
    ));

    let source = include_str!("embedded_ble.rs");
    let notify_start = source
        .find("fn open_notify_target()")
        .expect("notify target helper should exist");
    let notify_end = source[notify_start..]
        .find("fn open_notify_target_with_retry")
        .map(|offset| notify_start + offset)
        .expect("notify target helper boundary should exist");
    let notify_body = &source[notify_start..notify_end];

    assert!(notify_body.contains("native_windows_hid_pairing_addresses()"));
    assert!(notify_body.contains("native_windows_hid_snapshot_refresh_is_useful"));
    assert!(notify_body
        .contains("selected refreshed native Windows HID audio notify address={address:012X}"));
    assert!(
        !notify_body.contains("GetDeviceSelectorFromUuid(SERVICE_UUID)")
            || notify_body
                .find("native_windows_hid_pairing_addresses()")
                .expect("native HID identity refresh should occur")
                < notify_body
                    .find("GetDeviceSelectorFromUuid(SERVICE_UUID)")
                    .expect("service selector should remain after native HID handling"),
        "a changed current Windows HID identity must be retried directly before any system selector fallback"
    );
}

#[test]
fn native_windows_hid_current_identity_uses_random_address_type() {
    assert!(native_windows_hid_current_address_uses_random_identity(
        0xE4DE_5CBB_4A9B,
        &[0xE4DE_5CBB_4A9B],
    ));
    assert!(!native_windows_hid_current_address_uses_random_identity(
        0xE4DE_5CBB_4A9B,
        &[0xA4CB_8FF2_B510],
    ));

    let source = include_str!("embedded_ble.rs");
    assert!(source.contains("BluetoothLEDevice::FromBluetoothAddressAsync("));
    assert!(source.contains(
        "BluetoothLEDevice::GetDeviceSelectorFromBluetoothAddressWithBluetoothAddressType("
    ));
    assert!(source.contains("BluetoothAddressType::Random"));
}

#[test]
fn rename_recovery_preserves_advertised_address_type_through_first_pairasync() {
    let source = include_str!("embedded_ble.rs");
    let wait_start = source
        .find("pub(super) fn wait_for_bluetooth_target_advertisement_by_name")
        .expect("typed rename advertisement wait helper should exist");
    let wait_end = source[wait_start..]
        .find("fn find_bluetooth_target_service_address")
        .map(|offset| wait_start + offset)
        .expect("typed rename advertisement wait helper boundary should exist");
    let wait_body = &source[wait_start..wait_end];
    assert!(
        wait_body.contains("scan_listener_advertisements(context, Some(&target_name), timeout)")
            && wait_body.contains("remember_recovery_pairing_address_type(")
            && wait_body.contains("address_type"),
        "rename recovery must retain the address type from the exact-name advertisement"
    );

    let direct_start = source
        .find("fn listener_recovery_direct_pairing_candidates")
        .expect("direct recovery pairing helper should exist");
    let direct_end = source[direct_start..]
        .find("fn push_listener_pairing_candidate_if_matching")
        .map(|offset| direct_start + offset)
        .expect("direct recovery pairing helper boundary should exist");
    let direct_body = &source[direct_start..direct_end];
    assert!(
        direct_body.contains("recent_recovery_pairing_address_type(address)")
            && direct_body.contains("pairing_device_information_from_bluetooth_address_handle(")
            && direct_body.contains("path=typed_handle"),
        "the first fresh-address candidate must use the typed direct-handle path without a selector wait"
    );
    assert!(
        source.contains("BluetoothLEDevice::FromBluetoothAddressWithBluetoothAddressTypeAsync(")
            && source.contains("opened transient BLE DeviceInformation for pairing address={address_text} address_type={address_type:?}"),
        "typed identity must survive the transient WinRT device fallback and remain observable"
    );

    let custom = source
        .find("custom_pair_listener_candidate(pairing, &candidate.label, expected_name)")
        .expect("custom PairAsync should be the exact recovery ceremony");
    let standard = source
        .find("run_default_pairing_once(pairing, &candidate.label, \"standard pairing\")")
        .expect("generic standard PairAsync fallback should remain available");
    assert!(
        custom < standard,
        "typed Type recovery must register its ConfirmOnly handler before generic PairAsync"
    );
}

#[test]
fn persisted_startup_notify_state_ignores_legacy_or_wrong_target_address() {
    let legacy = PersistedBleDeviceState {
        last_successful_address: Some("FB8FBDD8C90F".to_string()),
        target_name: None,
        updated_at: None,
        last_ghost_prune_at: None,
        last_ghost_prune_keep_address: None,
    };
    assert_eq!(
        persisted_ble_device_state_address_for_target(&legacy, DEFAULT_BLUETOOTH_TARGET_NAME),
        None,
        "legacy address-only state must not be trusted after hardware swaps"
    );

    let wrong_target = PersistedBleDeviceState {
        last_successful_address: Some("FB8FBDD8C90F".to_string()),
        target_name: Some("OldType".to_string()),
        updated_at: None,
        last_ghost_prune_at: None,
        last_ghost_prune_keep_address: None,
    };
    assert_eq!(
        persisted_ble_device_state_address_for_target(&wrong_target, DEFAULT_BLUETOOTH_TARGET_NAME),
        None
    );

    let matching = PersistedBleDeviceState {
        last_successful_address: Some("E5:C3:D5:B8:D2:FC".to_string()),
        target_name: Some("listener".to_string()),
        updated_at: None,
        last_ghost_prune_at: None,
        last_ghost_prune_keep_address: None,
    };
    assert_eq!(
        persisted_ble_device_state_address_for_target(&matching, DEFAULT_BLUETOOTH_TARGET_NAME),
        Some(0xE5C3_D5B8_D2FC)
    );
}

#[test]
fn ghost_pairing_prune_cooldown_is_scoped_to_the_proven_keep_address() {
    let now = chrono::Utc::now();
    let state = PersistedBleDeviceState {
        last_successful_address: Some("DA:6C:ED:FB:55:40".to_string()),
        target_name: Some("listenerB".to_string()),
        updated_at: Some(now.to_rfc3339()),
        last_ghost_prune_at: Some((now - chrono::Duration::seconds(30)).to_rfc3339()),
        last_ghost_prune_keep_address: Some("DA:6C:ED:FB:55:40".to_string()),
    };
    let cooldown = Duration::from_secs(180);

    assert!(ghost_pairing_prune_state_is_recent_for_keep(
        &state,
        0xDA6C_EDFB_5540,
        now,
        cooldown,
    ));
    assert!(
        !ghost_pairing_prune_state_is_recent_for_keep(&state, 0xC32F_73D2_15B6, now, cooldown,),
        "a newly proven identity must not inherit the previous identity's cooldown"
    );

    let legacy_without_keep = PersistedBleDeviceState {
        last_ghost_prune_keep_address: None,
        ..state.clone()
    };
    assert!(!ghost_pairing_prune_state_is_recent_for_keep(
        &legacy_without_keep,
        0xDA6C_EDFB_5540,
        now,
        cooldown,
    ));
}

#[test]
fn advertisement_address_selection_does_not_short_circuit_on_runtime_cache() {
    let source = include_str!("embedded_ble.rs");
    let start = source
        .find("fn audio_target_advertisement_addresses")
        .expect("advertisement address helper should exist");
    let end = source[start..]
        .find("pub(super) fn remember_current_bluetooth_target_address_for_name")
        .map(|offset| start + offset)
        .expect("advertisement address helper boundary should exist");
    let body = &source[start..end];

    assert!(
        body.contains("configured_bluetooth_address_from_env()"),
        "only an explicit env-pinned BLE address may bypass fresh Windows discovery"
    );
    assert!(
        !body.contains("if let Some(address) = configured_bluetooth_address()"),
        "runtime cached BLE addresses must not short-circuit advertisement/PnP discovery"
    );
    assert!(
        body.contains("listener_pnp_service_signature_addresses()"),
        "current Windows PnP/service evidence must be considered before runtime cache"
    );
    assert!(
        body.contains("runtime_bluetooth_target_address()"),
        "runtime cache may remain a late fallback after fresh Windows candidates"
    );
}

#[test]
fn rapid_rename_predecessor_prefers_recent_pair_or_live_notify_before_windows_cache() {
    let source = include_str!("embedded_ble.rs");
    let start = source
        .find("pub(super) fn remember_current_bluetooth_target_address_for_name")
        .expect("rename predecessor helper should exist");
    let end = source[start..]
        .find("pub(super) fn wait_for_bluetooth_target_advertisement_by_name")
        .map(|offset| start + offset)
        .expect("rename predecessor helper boundary should exist");
    let body = &source[start..end];

    let recent = body
        .find("recent_listener_pairing_fast_gatt_address()")
        .expect("recent exact PairAsync address must be considered");
    let live = body
        .find(".or_else(runtime_bluetooth_target_address)")
        .expect("current runtime notify address must be the fallback authority");
    let windows = body
        .find("find_bluetooth_target_service_address(")
        .expect("Windows service discovery remains a late fallback");
    assert!(recent < live && live < windows);
    assert!(body.contains("retaining authoritative current/recent-pair address"));
}

#[test]
fn ble_candidate_filter_allows_trusted_address_when_windows_name_cache_lags() {
    let source = include_str!("embedded_ble.rs");
    let start = source
        .find("fn ble_candidate_allowed")
        .expect("BLE candidate filter should exist");
    let end = source[start..]
        .find("fn read_characteristic_bytes")
        .map(|offset| start + offset)
        .expect("BLE candidate filter boundary should exist");
    let body = &source[start..end];

    assert!(
        body.contains("listener_recovery_target_addresses()"),
        "Windows can expose the old BLE display name after a firmware rename; a trusted Listener address/service signature must still be allowed"
    );
    assert!(body.contains("trusted_addresses.contains(&address)"));
    assert!(body.contains("despite target name"));
}

#[test]
fn ble_candidate_filter_does_not_hard_reject_by_runtime_cache() {
    let source = include_str!("embedded_ble.rs");
    let start = source
        .find("fn ble_candidate_allowed")
        .expect("BLE candidate filter should exist");
    let end = source[start..]
        .find("fn read_characteristic_bytes")
        .map(|offset| start + offset)
        .expect("BLE candidate filter boundary should exist");
    let body = &source[start..end];

    assert!(
        body.contains("configured_bluetooth_address_from_env()"),
        "explicit env-pinned addresses may still hard-filter candidates"
    );
    assert!(
        !body.contains("configured_bluetooth_address()"),
        "runtime cached addresses must not hard-filter current Windows service candidates"
    );
    assert!(
        body.contains("listener_recovery_target_addresses()"),
        "Windows PnP/service addresses should remain a positive trust signal"
    );
}

#[test]
fn swift_pair_manufacturer_data_exposes_display_name() {
    assert_eq!(
        denzic_ble_windows::swift_pair_display_name_from_manufacturer_entry(
            0x0006,
            &[0x03, 0x00, 0x80, b'l', b'i', b's', b't', b'e', b'n', b'e', b'r', b'B']
        ),
        Some("listenerB".to_string())
    );
    assert_eq!(
        denzic_ble_windows::swift_pair_display_name_from_manufacturer_entry(
            0x0006,
            &[0x06, 0x00, 0x03, 0x00, 0x80, b'l', b'i', b's', b't', b'e', b'n', b'e', b'r', b'B']
        ),
        Some("listenerB".to_string())
    );
    assert_eq!(
        denzic_ble_windows::swift_pair_display_name_from_manufacturer_entry(
            0x004C,
            &[0x03, 0x00, 0x80, b'l', b'i', b's', b't', b'e', b'n', b'e', b'r', b'B']
        ),
        None
    );
}

#[test]
fn listener_target_name_set_does_not_fuzzy_match_listener_family() {
    let target_names = vec!["Blistener".to_string(), "OfficeType01".to_string()];
    assert!(bluetooth_name_matches_any("Blistener", &target_names));
    assert!(bluetooth_name_matches_any("OfficeType01", &target_names));
    assert!(!bluetooth_name_matches_any("listenerB", &target_names));
    assert!(!bluetooth_name_matches_any(
        "some-listener-device",
        &target_names
    ));
}

#[test]
fn recovery_advertisement_scan_prefers_configured_current_name() {
    let _guard = bluetooth_target_name_test_lock().lock().unwrap();
    let previous = configured_bluetooth_target_name();
    set_configured_bluetooth_target_name("listenerC");

    let scan_names = listener_recovery_advertisement_scan_names(&[
        "listenerB".to_string(),
        "listenerC".to_string(),
    ]);

    match previous {
        Some(name) => set_configured_bluetooth_target_name(&name),
        None => set_configured_bluetooth_target_name(""),
    }

    assert_eq!(
        scan_names,
        vec!["listenerC".to_string(), "listenerB".to_string()]
    );
}

#[test]
fn active_audio_control_registration_only_clears_matching_capture() {
    let _guard = active_audio_control_test_lock().lock().unwrap();
    *active_audio_control_slot().lock().unwrap() = None;

    let (tx1, _rx1) = mpsc::channel();
    let registration1 = ActiveAudioControlRegistration::install(10, tx1);
    assert_eq!(active_audio_control_sender().unwrap().capture_id, 10);

    let (tx2, _rx2) = mpsc::channel();
    let registration2 = ActiveAudioControlRegistration::install(20, tx2);
    assert_eq!(active_audio_control_sender().unwrap().capture_id, 20);

    drop(registration1);
    assert_eq!(active_audio_control_sender().unwrap().capture_id, 20);

    drop(registration2);
    assert!(active_audio_control_sender().is_none());
}

#[test]
fn device_settings_command_prefers_active_capture() {
    let _guard = active_audio_control_test_lock().lock().unwrap();
    *active_audio_control_slot().lock().unwrap() = None;

    let (tx, rx) = mpsc::channel();
    let registration = ActiveAudioControlRegistration::install(42, tx);
    let receiver = std::thread::spawn(move || {
        let request = rx
            .recv_timeout(Duration::from_secs(1))
            .expect("active control request");
        assert_eq!(request.bytes, b"DEVICE:SET knob_rotation=system_volume\n");
        assert_eq!(request.label, "device settings");
        request.result_tx.send(Ok(())).expect("send result");
    });

    send_device_settings_command(
        "DEVICE:SET knob_rotation=system_volume",
        Duration::from_millis(200),
    )
    .expect("device settings command should use active capture");

    receiver.join().expect("receiver thread");
    drop(registration);
    assert!(active_audio_control_sender().is_none());
}

#[test]
fn device_settings_ble_name_commands_skip_active_capture() {
    assert!(!device_settings_command_allows_active_capture(
        "DEVICE:SET ble_name=Blistener"
    ));
    assert!(!device_settings_command_allows_active_capture(
        "DEVICE:SET name=Blistener"
    ));
    assert!(device_settings_command_allows_active_capture(
        "DEVICE:SET knob_rotation=system_volume"
    ));
    assert!(device_settings_command_allows_active_capture(
        "DEVICE:STATUS"
    ));
    assert_eq!(
        device_settings_command_ble_name_target("DEVICE:SET ble_name=Blistener"),
        Some("Blistener".to_string())
    );
    assert_eq!(
        device_settings_command_ble_name_target("DEVICE:SET name=OfficeType01"),
        Some("OfficeType01".to_string())
    );
    assert_eq!(
        device_settings_command_ble_name_target("DEVICE:SET knob_rotation=system_volume"),
        None
    );
}

#[test]
fn runtime_target_address_allows_windows_name_cache_mismatch() {
    let _guard = bluetooth_target_name_test_lock().lock().unwrap();
    if configured_bluetooth_address_from_env().is_some()
        || std::env::var("LISTENER_TYPE_BLE_TARGET_NAME").is_ok()
        || std::env::var("LISTENER_TYPE_BLUETOOTH_TARGET_NAME").is_ok()
    {
        return;
    }

    let previous_name = configured_bluetooth_target_name();
    let previous_address = RUNTIME_BLUETOOTH_TARGET_ADDRESS
        .get_or_init(|| Mutex::new(None))
        .lock()
        .ok()
        .and_then(|slot| slot.clone());

    set_configured_bluetooth_target_name("OfficeType01");
    remember_runtime_bluetooth_target_address_for_name(
        0xA4CB8FF2B512,
        "OfficeType01",
        BLE_RENAME_ADDRESS_GRACE_WINDOW,
        "unit test",
    );

    assert!(ble_candidate_allowed(
        "test",
        0,
        "Blistener",
        Some(0xA4CB8FF2B512)
    ));
    assert!(!ble_candidate_allowed(
        "test",
        1,
        "Blistener",
        Some(0xA4CB8FF2B513)
    ));

    match previous_name {
        Some(name) => set_configured_bluetooth_target_name(&name),
        None => set_configured_bluetooth_target_name(""),
    }
    if let Ok(mut slot) = RUNTIME_BLUETOOTH_TARGET_ADDRESS
        .get_or_init(|| Mutex::new(None))
        .lock()
    {
        *slot = previous_address;
    }
}

#[test]
fn verified_rename_handoff_address_requires_matching_name_and_rename_grace() {
    let _guard = bluetooth_target_name_test_lock().lock().unwrap();
    if configured_bluetooth_address_from_env().is_some()
        || std::env::var("LISTENER_TYPE_BLE_TARGET_NAME").is_ok()
        || std::env::var("LISTENER_TYPE_BLUETOOTH_TARGET_NAME").is_ok()
    {
        return;
    }

    let previous_name = configured_bluetooth_target_name();
    let previous_address = RUNTIME_BLUETOOTH_TARGET_ADDRESS
        .get_or_init(|| Mutex::new(None))
        .lock()
        .ok()
        .and_then(|slot| slot.clone());

    set_configured_bluetooth_target_name("RenameTarget");
    remember_runtime_bluetooth_target_address_for_name(
        0xA4CB8FF2B512,
        "RenameTarget",
        BLE_RENAME_ADDRESS_GRACE_WINDOW,
        "unit test rename handoff",
    );
    assert_eq!(
        verified_bluetooth_target_rename_handoff_address(),
        Some(0xA4CB8FF2B512),
        "a current target address learned for the rename grace window is safe only after firmware confirms the name transition"
    );

    remember_runtime_bluetooth_target_address_for_name(
        0xA4CB8FF2B512,
        "RenameTarget",
        BLE_TARGET_ADDRESS_CACHE_WINDOW,
        "unit test ordinary cache",
    );
    assert_eq!(
        verified_bluetooth_target_rename_handoff_address(),
        None,
        "an ordinary runtime cache address must not become a rename recovery shortcut"
    );

    match previous_name {
        Some(name) => set_configured_bluetooth_target_name(&name),
        None => set_configured_bluetooth_target_name(""),
    }
    if let Ok(mut slot) = RUNTIME_BLUETOOTH_TARGET_ADDRESS
        .get_or_init(|| Mutex::new(None))
        .lock()
    {
        *slot = previous_address;
    }
}

#[test]
fn default_target_name_accepts_discovered_custom_listener_service_name() {
    let _guard = bluetooth_target_name_test_lock().lock().unwrap();
    if configured_bluetooth_address_from_env().is_some()
        || std::env::var("LISTENER_TYPE_BLE_TARGET_NAME").is_ok()
        || std::env::var("LISTENER_TYPE_BLUETOOTH_TARGET_NAME").is_ok()
    {
        return;
    }

    let previous_name = configured_bluetooth_target_name();
    set_configured_bluetooth_target_name(DEFAULT_BLUETOOTH_TARGET_NAME);

    assert!(ble_candidate_allowed(
        "test",
        0,
        "Blistener",
        Some(0xA4CB8FF2B512)
    ));
    assert!(!ble_candidate_allowed("test", 1, "", Some(0xA4CB8FF2B512)));

    match previous_name {
        Some(name) => set_configured_bluetooth_target_name(&name),
        None => set_configured_bluetooth_target_name(""),
    }
}

#[test]
fn candidate_address_learning_promotes_default_target_name_to_discovered_name() {
    let (target_name, update_configured_name) = bluetooth_target_name_for_candidate(
        Some(DEFAULT_BLUETOOTH_TARGET_NAME.to_string()),
        Some("Blistener".to_string()),
    );

    assert_eq!(target_name, "Blistener");
    assert!(update_configured_name);
}

#[test]
fn recording_stop_uses_bounded_active_then_usb_serial_without_fresh_gatt() {
    assert_eq!(
        bounded_recording_stop_active_timeout(Duration::from_secs(5)),
        RECORDING_STOP_ACTIVE_CONTROL_TIMEOUT
    );
    let source = include_str!("embedded_ble.rs");
    let start = source
        .find("fn send_recording_stop_control_command")
        .expect("recording stop helper should exist");
    let end = source[start..]
        .find("pub fn send_recording_control_toggle")
        .map(|offset| start + offset)
        .expect("recording stop helper boundary should exist");
    let body = &source[start..end];

    assert!(body.contains("send_audio_control_via_active_capture"));
    assert!(body.contains("send_control_command_via_usb_serial(\"VREC:STOP\""));
    assert!(
        !body.contains("BleFreshGattGuard::enter"),
        "recording stop must not open a late fresh GATT path while active audio is streaming"
    );
    assert!(
        !body.contains("open_audio_control_target_with_retry"),
        "recording stop must not run the long audio-control rediscovery path"
    );
}

#[test]
fn processing_hints_do_not_retry_with_late_fresh_gatt() {
    assert_eq!(
        processing_state_active_transient_fallback(true),
        ActiveControlTransientFallback::ReturnError
    );
    assert_eq!(
        processing_state_active_transient_fallback(false),
        ActiveControlTransientFallback::ReturnError
    );
}

#[test]
fn foreground_probe_success_disables_notify_cccd() {
    assert_eq!(
        NotifyCccdTeardown::for_probe_success(),
        NotifyCccdTeardown::Disable
    );
}

#[test]
fn ota_background_capture_cancel_leaves_old_cccd_for_connection_handoff() {
    assert_eq!(
        NotifyCccdTeardown::for_capture_cancel(
            CaptureTerminalBehavior::ContinueListening,
            true,
            false,
        ),
        NotifyCccdTeardown::LeaveEnabled
    );
    assert_eq!(
        NotifyCccdTeardown::for_capture_cancel(
            CaptureTerminalBehavior::ContinueListening,
            false,
            false,
        ),
        NotifyCccdTeardown::Disable
    );
    assert_eq!(
        NotifyCccdTeardown::for_capture_cancel(CaptureTerminalBehavior::StopCapture, true, false,),
        NotifyCccdTeardown::Disable
    );
}

#[test]
fn confirmed_name_change_handoff_skips_old_cccd_only_for_continuous_listener() {
    assert_eq!(
        NotifyCccdTeardown::for_capture_cancel(
            CaptureTerminalBehavior::ContinueListening,
            false,
            true,
        ),
        NotifyCccdTeardown::LeaveEnabled,
        "a firmware-confirmed BLE rename terminates the old connection, so its background listener must not wait on an obsolete CCCD disable"
    );
    assert_eq!(
        NotifyCccdTeardown::for_capture_cancel(CaptureTerminalBehavior::StopCapture, false, true,),
        NotifyCccdTeardown::Disable,
        "foreground capture cancellation still tears down its CCCD normally"
    );
}

#[test]
fn winrt_bluetooth_targets_release_without_synchronous_close_after_handler_detach() {
    let source = include_str!("embedded_ble.rs");

    assert!(
        source.contains("fn release_winrt_bluetooth_object<T>(object: T)")
            && source.contains("dropping the COM reference lets WinRT finish"),
        "WinRT Bluetooth target release policy must remain explicit"
    );
    assert!(
        source.contains("last_ghost_prune_at")
            && source.contains("last_ghost_prune_keep_address")
            && source.contains("persisted_ghost_pairing_prune_is_recent_for_keep")
            && source.contains("persisted cross-process cooldown active"),
        "automatic ghost pairing cleanup must retain its address-scoped cooldown across Type restarts"
    );
    for forbidden in ["session.Close()", "service.Close()", "device.Close()"] {
        assert!(
            !source.contains(forbidden),
            "WinRT Bluetooth target cleanup must not synchronously call {forbidden}"
        );
    }

    let finish_start = source
        .find("fn finish(&mut self, teardown: NotifyCccdTeardown)")
        .expect("notify cleanup finish should exist");
    let finish_end = source[finish_start..]
        .find("fn handle_audio_control_request")
        .map(|offset| finish_start + offset)
        .expect("notify cleanup finish boundary should exist");
    let finish = &source[finish_start..finish_end];
    let remove_status = finish
        .find("self.remove_status_handlers();")
        .expect("status handlers must be removed");
    let remove_value = finish
        .find("self.remove_handler();")
        .expect("ValueChanged handler must be removed");
    let release_registration = finish
        .find("self.audio_control_registration.take()")
        .expect("active control registration must be released");
    assert!(
        remove_status < remove_value && remove_value < release_registration,
        "notify cleanup must detach WinRT event sources before releasing retained target state"
    );
}

#[test]
fn transient_gatt_inactive_does_not_tear_down_a_connected_notify_target() {
    let source = include_str!("embedded_ble.rs");
    let handler_start = source
        .find("fn register_gatt_session_status_handler")
        .expect("GATT session status handler should exist");
    let handler_end = source[handler_start..]
        .find("struct OpenListenerOtaV1Target")
        .map(|offset| handler_start + offset)
        .expect("GATT session status handler boundary should exist");
    let handler = &source[handler_start..handler_end];

    assert!(handler.contains("BleCaptureSignal::GattSessionInactive"));
    assert!(handler.contains("BleCaptureSignal::GattSessionActive"));
    assert!(
        !handler.contains("BleCaptureSignal::Disconnected"),
        "GATT Inactive must not be treated as a physical BluetoothLEDevice disconnect"
    );

    let inactive_branch = source
        .find("BleCaptureSignal::GattSessionInactive(reason) =>")
        .expect("capture wait should handle advisory GATT Inactive");
    let active_branch = source[inactive_branch..]
        .find("BleCaptureSignal::GattSessionActive =>")
        .map(|offset| inactive_branch + offset)
        .expect("advisory GATT Inactive branch boundary should exist");
    let body = &source[inactive_branch..active_branch];
    assert!(body.contains("retaining the notify target"));
    assert!(body.contains("continue;"));
    assert!(
        !body.contains("cleanup.disable_notify()"),
        "transient GATT Inactive must preserve the existing notify channel"
    );
}
