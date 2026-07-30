//! Embedded BLE audio submit + pairing recovery surface.

use super::super::CoordinatorState;
use std::path::PathBuf;
use std::time::Duration;

use serde::Serialize;

use crate::coordinator::{
    Coordinator, EmbeddedBleNotifySubscriptionState, EmbeddedBleWakeRecoverySnapshot,
};

pub async fn submit_embedded_audio_notifications(
    coord: CoordinatorState<'_>,
    notifications: Vec<Vec<u8>>,
) -> Result<crate::embedded_audio::EmbeddedAudioSubmissionResult, String> {
    coord
        .submit_embedded_audio_notifications(notifications)
        .await
}

pub async fn submit_embedded_audio_streaming_notifications(
    coord: CoordinatorState<'_>,
    notifications: Vec<Vec<u8>>,
) -> Result<crate::embedded_audio::EmbeddedAudioSubmissionResult, String> {
    coord
        .submit_embedded_audio_streaming_notifications(notifications)
        .await
}

pub async fn submit_embedded_audio_file(
    coord: CoordinatorState<'_>,
    path: String,
    format: Option<crate::embedded_audio::EmbeddedAudioInputFormat>,
) -> Result<crate::embedded_audio::EmbeddedAudioSubmissionResult, String> {
    coord
        .submit_embedded_audio_file(PathBuf::from(path), format)
        .await
}

pub async fn submit_embedded_audio_streaming_file(
    coord: CoordinatorState<'_>,
    path: String,
    format: Option<crate::embedded_audio::EmbeddedAudioInputFormat>,
) -> Result<crate::embedded_audio::EmbeddedAudioSubmissionResult, String> {
    coord
        .submit_embedded_audio_streaming_file(PathBuf::from(path), format)
        .await
}

pub async fn submit_embedded_audio_ble_once(
    coord: CoordinatorState<'_>,
    timeout_ms: Option<u64>,
) -> Result<crate::embedded_audio::EmbeddedAudioSubmissionResult, String> {
    coord.submit_embedded_audio_ble_once(timeout_ms).await
}

pub async fn probe_embedded_audio_ble_subscription(
    coord: CoordinatorState<'_>,
    timeout_ms: Option<u64>,
) -> Result<(), String> {
    coord
        .probe_embedded_audio_ble_subscription(timeout_ms)
        .await
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddedBleRepairResult {
    pub recovered: bool,
    pub user_action_required: bool,
    pub open_bluetooth_settings: bool,
    pub recovery_action: EmbeddedBleRecoveryAction,
    pub message: String,
    pub failure: Option<crate::embedded_ble::BleFailureClassification>,
    pub unpair_result: Option<crate::embedded_ble::BleDeviceUnpairResult>,
    pub pairing_prompt_result: Option<crate::embedded_ble::BleDevicePairingPromptResult>,
    pub runtime: EmbeddedBleRuntimeStatus,
    pub firmware: crate::embedded_ble::FirmwareOtaDeviceSnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum EmbeddedBleRecoveryAction {
    None,
    Reconnected,
    WaitForAutomaticRecovery,
    RePairRequired,
    BluetoothSettingsRequired,
    DiagnosticsRequired,
}

pub fn embedded_ble_repair_failure_action(
    failure: &crate::embedded_ble::BleFailureClassification,
) -> (bool, bool) {
    let open_bluetooth_settings = matches!(
        failure.kind,
        crate::embedded_ble::BleFailureKind::DeviceMissing
            | crate::embedded_ble::BleFailureKind::MissingPairing
            | crate::embedded_ble::BleFailureKind::StaleGattService
            | crate::embedded_ble::BleFailureKind::CccdProtocolError
            | crate::embedded_ble::BleFailureKind::WindowsBluetoothServiceResetNeeded
            | crate::embedded_ble::BleFailureKind::AccessDenied
    );
    let user_action_required = open_bluetooth_settings || !failure.automatic_recovery;
    (user_action_required, open_bluetooth_settings)
}

pub fn embedded_ble_recovery_action_for_failure(
    failure: &crate::embedded_ble::BleFailureClassification,
) -> EmbeddedBleRecoveryAction {
    match failure.kind {
        crate::embedded_ble::BleFailureKind::MissingPairing
        | crate::embedded_ble::BleFailureKind::StaleGattService
        | crate::embedded_ble::BleFailureKind::CccdProtocolError => {
            EmbeddedBleRecoveryAction::RePairRequired
        }
        crate::embedded_ble::BleFailureKind::WindowsBluetoothServiceResetNeeded
        | crate::embedded_ble::BleFailureKind::AccessDenied
        | crate::embedded_ble::BleFailureKind::DeviceMissing => {
            EmbeddedBleRecoveryAction::BluetoothSettingsRequired
        }
        crate::embedded_ble::BleFailureKind::LowPowerIdleDisconnect
        | crate::embedded_ble::BleFailureKind::PairedButDisconnected
        | crate::embedded_ble::BleFailureKind::BackgroundListenerContention
        | crate::embedded_ble::BleFailureKind::OtaRebootWindow => {
            EmbeddedBleRecoveryAction::WaitForAutomaticRecovery
        }
        crate::embedded_ble::BleFailureKind::DeviceAsleep => {
            EmbeddedBleRecoveryAction::WaitForAutomaticRecovery
        }
        crate::embedded_ble::BleFailureKind::MissingDisFirmwareRevision
        | crate::embedded_ble::BleFailureKind::UnsupportedPlatform
        | crate::embedded_ble::BleFailureKind::Unknown => {
            EmbeddedBleRecoveryAction::DiagnosticsRequired
        }
    }
}

pub fn should_attempt_embedded_ble_auto_unpair(
    failure: &crate::embedded_ble::BleFailureClassification,
) -> bool {
    matches!(
        failure.kind,
        crate::embedded_ble::BleFailureKind::MissingPairing
            | crate::embedded_ble::BleFailureKind::StaleGattService
            | crate::embedded_ble::BleFailureKind::CccdProtocolError
    )
}

pub fn runtime_suggests_embedded_ble_auto_unpair(
    repair_error: &str,
    listener_last_error: Option<&str>,
    wake_recovery: &EmbeddedBleWakeRecoverySnapshot,
) -> bool {
    if wake_recovery.reconnect_attempts < 3 {
        return false;
    }

    if !matches!(
        wake_recovery.notify_subscription_state,
        EmbeddedBleNotifySubscriptionState::Failed
            | EmbeddedBleNotifySubscriptionState::Opening
            | EmbeddedBleNotifySubscriptionState::Lost
            | EmbeddedBleNotifySubscriptionState::Unknown
    ) {
        return false;
    }

    let combined = [
        Some(repair_error),
        listener_last_error,
        wake_recovery.recent_disconnect_reason.as_deref(),
        Some(wake_recovery.user_guidance.as_str()),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" ")
    .to_ascii_lowercase();

    let low_power_idle = wake_recovery.usb_powered == Some(false)
        && runtime_error_text_suggests_low_power_idle(&combined);
    let notify_or_gatt_failure = combined.contains("cccd")
        || combined.contains("notify write")
        || combined.contains("notify subscription")
        || combined.contains("gatt session still not active")
        || combined.contains("gattsessionstatus(0)")
        || combined.contains("bluetoothconnectionstatus(0)");
    let timeout_like = combined.contains("timed out")
        || combined.contains("timeout")
        || combined.contains("not active")
        || combined.contains("not recover")
        || combined.contains("did not recover")
        || combined.contains("disconnected");

    notify_or_gatt_failure && timeout_like && !low_power_idle
}

pub fn runtime_error_text_suggests_low_power_idle(combined: &str) -> bool {
    let reason_546 = combined.contains("reason=546")
        || combined.contains("reason: 546")
        || combined.contains("reason 546");
    let idle_label = combined.contains("low-power idle")
        || combined.contains("low power idle")
        || combined.contains("idle disconnect");
    let transport_not_ready =
        combined.contains("transport_not_ready") || combined.contains("transport not ready");
    let link_loss = combined.contains("connection status changed")
        || combined.contains("gatt session status changed")
        || combined.contains("disconnected");

    reason_546 || idle_label || (transport_not_ready && link_loss)
}

pub fn embedded_ble_recovery_message(
    failure: &crate::embedded_ble::BleFailureClassification,
    unpair_result: Option<&crate::embedded_ble::BleDeviceUnpairResult>,
    pairing_prompt_result: Option<&crate::embedded_ble::BleDevicePairingPromptResult>,
) -> String {
    if let Some(pairing) = pairing_prompt_result {
        match pairing.status {
            crate::embedded_ble::BleDevicePairingPromptStatus::Paired => {
                return "Windows 已完成 Listener 配对。Type 正在重新连接；如果状态没有恢复，请再点一次重试。".to_string();
            }
            crate::embedded_ble::BleDevicePairingPromptStatus::AlreadyPaired => {
                return "Listener 在 Windows 里已经是已配对状态。Type 正在重新连接；如果仍失败，请重新打开蓝牙设置检查连接。".to_string();
            }
            crate::embedded_ble::BleDevicePairingPromptStatus::NotFound => {
                return "Type 没有看到可自动配对的 Listener。如果另一台电脑已经用 Windows 弹窗连上，这是预期；否则请保持设备可配对后重试。".to_string();
            }
            crate::embedded_ble::BleDevicePairingPromptStatus::NeedsUserAction => {
                return "Type 已尝试本机自动恢复但 Windows 未完成配对。如果另一台电脑已经连上，本机会停止抢回；否则请保持设备可配对后重试。".to_string();
            }
        }
    }
    if let Some(unpair) = unpair_result {
        return match unpair.status {
            crate::embedded_ble::BleDeviceUnpairStatus::Removed => {
                "Type 已清理这台电脑上的旧 Listener 配对，接下来会尝试本机自动恢复；如果另一台电脑已经连上，本机会停止抢回。".to_string()
            }
            crate::embedded_ble::BleDeviceUnpairStatus::AlreadyClean
            | crate::embedded_ble::BleDeviceUnpairStatus::NotFound => {
                "这台电脑没有可自动清理的旧 Listener 配对。如果要换电脑，请在另一台电脑用 Windows 弹窗连接；如果要恢复本机，请保持设备可配对后重试。".to_string()
            }
            crate::embedded_ble::BleDeviceUnpairStatus::NeedsUserAction => {
                "Windows 没有允许 Type 自动清理旧 Listener 配对。请先在 Windows 蓝牙里删除旧设备，再点击连接通知或手动添加 Listener。".to_string()
            }
        };
    }
    match embedded_ble_recovery_action_for_failure(failure) {
        EmbeddedBleRecoveryAction::WaitForAutomaticRecovery => {
            "Listener Type 正在自动恢复连接。请保持设备唤醒，稍等片刻。".to_string()
        }
        EmbeddedBleRecoveryAction::BluetoothSettingsRequired => {
            "请在打开的 Windows 蓝牙设置里确认 Listener 已连接；如果仍失败，请删除后重新配对。"
                .to_string()
        }
        EmbeddedBleRecoveryAction::DiagnosticsRequired => {
            "Type 不能自动恢复这个状态。请导出诊断包给支持人员。".to_string()
        }
        EmbeddedBleRecoveryAction::RePairRequired => {
            "请重新配对 Listener；Type 会继续检测并自动恢复。".to_string()
        }
        EmbeddedBleRecoveryAction::None | EmbeddedBleRecoveryAction::Reconnected => {
            failure.user_action.to_string()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddedBleWindowsPairingPromptPolicy {
    AllowUserPrompt,
    SuppressUserPrompt,
}

impl EmbeddedBleWindowsPairingPromptPolicy {
    fn allows_user_prompt(self) -> bool {
        matches!(self, Self::AllowUserPrompt)
    }
}

pub fn embedded_ble_windows_pairing_result(
    context: &str,
    expected_ble_name: &str,
    type_recovery_command_confirmed: bool,
    prompt_policy: EmbeddedBleWindowsPairingPromptPolicy,
    pre_pair_stale_cleanup: bool,
    observed_recovery_addresses: &[u64],
) -> crate::embedded_ble::BleDevicePairingPromptResult {
    let pairing = if prompt_policy.allows_user_prompt() {
        if type_recovery_command_confirmed {
            crate::embedded_ble::prompt_listener_pairing_after_type_recovery(Some(
                expected_ble_name,
            ))
        } else {
            crate::embedded_ble::prompt_listener_pairing_for_recovery(Some(expected_ble_name))
        }
    } else if type_recovery_command_confirmed && !pre_pair_stale_cleanup {
        crate::embedded_ble::prompt_listener_pairing_after_type_recovery_without_user_prompt_after_cache_cleanup_for_addresses(
            Some(expected_ble_name),
            observed_recovery_addresses,
        )
    } else if type_recovery_command_confirmed {
        crate::embedded_ble::prompt_listener_pairing_after_type_recovery_without_user_prompt(Some(
            expected_ble_name,
        ))
    } else {
        crate::embedded_ble::prompt_listener_pairing_for_recovery_without_user_prompt(Some(
            expected_ble_name,
        ))
    };
    log::info!(
        "[embedded-ble] {context} Windows PairAsync status={:?} matched={} paired_now={} already_paired={} failed={} open_settings={} recovery_command_confirmed={type_recovery_command_confirmed} allow_user_prompt={}",
        pairing.status,
        pairing.matched_devices,
        pairing.prompted_devices,
        pairing.already_paired_devices,
        pairing.failed_devices,
        pairing.open_bluetooth_settings,
        prompt_policy.allows_user_prompt(),
    );
    pairing
}

pub async fn embedded_ble_runtime_and_firmware(
    coord: &Coordinator,
) -> Result<
    (
        EmbeddedBleRuntimeStatus,
        crate::embedded_ble::FirmwareOtaDeviceSnapshot,
    ),
    String,
> {
    let firmware =
        tauri::async_runtime::spawn_blocking(crate::embedded_ble::firmware_ota_device_snapshot)
            .await
            .map_err(|err| format!("Listener BLE repair snapshot task failed: {err}"))?;
    coord.record_embedded_ble_firmware_power_snapshot(&firmware, "runtime_and_firmware");
    let runtime = EmbeddedBleRuntimeStatus {
        background_listener_disabled_by_env: std::env::var("LISTENER_TYPE_DISABLE_BACKGROUND_BLE")
            .ok()
            .is_some_and(|value| value == "1"),
        background_listener_active: coord.embedded_ble_listener_active(),
        background_listener_ready: coord.embedded_ble_listener_ready(),
        background_listener_generation: coord.embedded_ble_listener_generation(),
        background_listener_last_error: coord.embedded_ble_listener_last_error(),
        wake_recovery: coord.embedded_ble_wake_recovery_snapshot(),
    };
    Ok((runtime, firmware))
}

pub async fn repair_embedded_ble_connection(
    coord: CoordinatorState<'_>,
    timeout_ms: Option<u64>,
) -> Result<EmbeddedBleRepairResult, String> {
    let repair = coord.repair_embedded_ble_connection(timeout_ms).await;
    let (runtime, firmware) = embedded_ble_runtime_and_firmware(&coord).await?;

    match repair {
        Ok(snapshot) => Ok(EmbeddedBleRepairResult {
            recovered: true,
            user_action_required: false,
            open_bluetooth_settings: false,
            recovery_action: EmbeddedBleRecoveryAction::Reconnected,
            message: snapshot.user_guidance,
            failure: None,
            unpair_result: None,
            pairing_prompt_result: None,
            runtime,
            firmware,
        }),
        Err(err) => {
            let failure = crate::embedded_ble::classify_ble_failure(&err);
            let (user_action_required, open_bluetooth_settings) =
                embedded_ble_repair_failure_action(&failure);
            let recovery_action = embedded_ble_recovery_action_for_failure(&failure);
            Ok(EmbeddedBleRepairResult {
                recovered: false,
                user_action_required,
                open_bluetooth_settings,
                recovery_action,
                message: embedded_ble_recovery_message(&failure, None, None),
                failure: Some(failure),
                unpair_result: None,
                pairing_prompt_result: None,
                runtime,
                firmware,
            })
        }
    }
}

pub async fn recover_embedded_ble_device(
    coord: CoordinatorState<'_>,
    timeout_ms: Option<u64>,
) -> Result<EmbeddedBleRepairResult, String> {
    let repair = coord.repair_embedded_ble_connection(timeout_ms).await;
    match repair {
        Ok(snapshot) => {
            coord.clear_embedded_ble_pairing_confirmation_hold("one-click recovery repaired");
            let (runtime, firmware) = embedded_ble_runtime_and_firmware(&coord).await?;
            Ok(EmbeddedBleRepairResult {
                recovered: true,
                user_action_required: false,
                open_bluetooth_settings: false,
                recovery_action: EmbeddedBleRecoveryAction::Reconnected,
                message: snapshot.user_guidance,
                failure: None,
                unpair_result: None,
                pairing_prompt_result: None,
                runtime,
                firmware,
            })
        }
        Err(err) => {
            let failure = crate::embedded_ble::classify_ble_failure(&err);
            let (mut user_action_required, mut open_bluetooth_settings) =
                embedded_ble_repair_failure_action(&failure);
            let mut recovery_action = embedded_ble_recovery_action_for_failure(&failure);
            let unpair_result = None;
            let mut pairing_prompt_result = None;
            let listener_last_error = coord.embedded_ble_listener_last_error();
            let wake_recovery = coord.embedded_ble_wake_recovery_snapshot();
            let runtime_requests_auto_unpair = runtime_suggests_embedded_ble_auto_unpair(
                &err,
                listener_last_error.as_deref(),
                &wake_recovery,
            );

            if should_attempt_embedded_ble_auto_unpair(&failure) || runtime_requests_auto_unpair {
                let cleanup_wait_timeout = Duration::from_secs(45);
                log::info!(
                    "[embedded-ble] one-click recovery waiting for active Listener capture to stop before device cleanup timeout_ms={}",
                    cleanup_wait_timeout.as_millis()
                );
                let capture_stopped = coord
                    .pause_embedded_ble_listener_for_recovery_cleanup(cleanup_wait_timeout)
                    .await;
                log::info!(
                    "[embedded-ble] one-click recovery capture stop before device cleanup stopped={capture_stopped}"
                );
                let expected_ble_name = coord.prefs().get().device_ble_name;
                coord.hold_embedded_ble_listener_for_native_pairing_handoff(
                    expected_ble_name.clone(),
                );
                log::info!(
                    "[embedded-ble] one-click recovery attempting Type automatic PairAsync recovery failure_kind={:?} runtime_escalated={runtime_requests_auto_unpair} reconnect_attempts={} notify_state={:?}",
                    failure.kind,
                    wake_recovery.reconnect_attempts,
                    wake_recovery.notify_subscription_state,
                );
                let pairing_expected_name = expected_ble_name.clone();
                let pairing = tauri::async_runtime::spawn_blocking(move || {
                    embedded_ble_windows_pairing_result(
                        "one-click recovery",
                        pairing_expected_name.as_str(),
                        true,
                        EmbeddedBleWindowsPairingPromptPolicy::AllowUserPrompt,
                        true,
                        &[],
                    )
                })
                .await
                .map_err(|err| {
                    format!("Listener BLE Type automatic PairAsync task failed: {err}")
                })?;
                let pairing_ready = matches!(
                    pairing.status,
                    crate::embedded_ble::BleDevicePairingPromptStatus::Paired
                        | crate::embedded_ble::BleDevicePairingPromptStatus::AlreadyPaired
                ) && !pairing.open_bluetooth_settings
                    && pairing.failed_devices == 0;
                log::info!(
                    "[embedded-ble] one-click recovery Type PairAsync requested pairing_ready={pairing_ready} target={expected_ble_name:?}"
                );
                user_action_required = false;
                recovery_action = if pairing_ready {
                    EmbeddedBleRecoveryAction::WaitForAutomaticRecovery
                } else {
                    EmbeddedBleRecoveryAction::RePairRequired
                };
                open_bluetooth_settings = false;
                pairing_prompt_result = Some(pairing.clone());
                log::info!(
                    "[embedded-ble] one-click recovery will stop if another host completes pairing before this Type instance target={expected_ble_name:?}"
                );
            }

            let (runtime, firmware) = embedded_ble_runtime_and_firmware(&coord).await?;
            Ok(EmbeddedBleRepairResult {
                recovered: false,
                user_action_required,
                open_bluetooth_settings,
                recovery_action,
                message: embedded_ble_recovery_message(
                    &failure,
                    unpair_result.as_ref(),
                    pairing_prompt_result.as_ref(),
                ),
                failure: Some(failure),
                unpair_result,
                pairing_prompt_result,
                runtime,
                firmware,
            })
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddedBleRuntimeStatus {
    pub background_listener_disabled_by_env: bool,
    pub background_listener_active: bool,
    pub background_listener_ready: bool,
    pub background_listener_generation: u64,
    pub background_listener_last_error: Option<String>,
    pub wake_recovery: EmbeddedBleWakeRecoverySnapshot,
}

pub fn get_embedded_ble_runtime_status(coord: CoordinatorState<'_>) -> EmbeddedBleRuntimeStatus {
    EmbeddedBleRuntimeStatus {
        background_listener_disabled_by_env: std::env::var("LISTENER_TYPE_DISABLE_BACKGROUND_BLE")
            .ok()
            .is_some_and(|value| value == "1"),
        background_listener_active: coord.embedded_ble_listener_active(),
        background_listener_ready: coord.embedded_ble_listener_ready(),
        background_listener_generation: coord.embedded_ble_listener_generation(),
        background_listener_last_error: coord.embedded_ble_listener_last_error(),
        wake_recovery: coord.embedded_ble_wake_recovery_snapshot(),
    }
}

pub async fn submit_embedded_audio_ble_stream(
    coord: CoordinatorState<'_>,
    timeout_ms: Option<u64>,
) -> Result<crate::embedded_audio::EmbeddedAudioSubmissionResult, String> {
    coord.submit_embedded_audio_ble_stream(timeout_ms).await
}
