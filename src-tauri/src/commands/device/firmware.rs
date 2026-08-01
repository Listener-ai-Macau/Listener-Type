//! Firmware OTA (BLE) + wired factory flash surface.

use super::super::CoordinatorState;
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use espflash::connection::reset::{ResetAfterOperation, ResetBeforeOperation};
use espflash::elf::RomSegment;
use espflash::flasher::{FlashFrequency, FlashMode, FlashSize, Flasher, ProgressCallbacks};
pub use espflash::targets::Chip;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use serialport::{FlowControl, SerialPortType, UsbPortInfo};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter};

use crate::coordinator_state::SessionPhase;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FirmwareOtaPreflightSnapshot {
    pub recording_active: bool,
    pub dictation_phase: String,
    pub device: crate::embedded_ble::FirmwareOtaDeviceSnapshot,
    pub handoff_elapsed_ms: Option<u64>,
    pub target_probe_elapsed_ms: Option<u64>,
    pub total_elapsed_ms: u64,
}

pub const FIRMWARE_OTA_LISTENER_V1_GATT_PROBE_TIMEOUT: Duration = Duration::from_secs(8);
pub const FIRMWARE_OTA_LISTENER_V1_PREFLIGHT_TIMEOUT: Duration = Duration::from_secs(10);

pub fn firmware_ota_preflight_unavailable_snapshot(
    detail: impl Into<String>,
) -> crate::embedded_ble::FirmwareOtaDeviceSnapshot {
    crate::embedded_ble::FirmwareOtaDeviceSnapshot {
        connected: false,
        hardware_revision: None,
        firmware_version: None,
        capabilities: Vec::new(),
        battery_percent: None,
        usb_powered: None,
        detail: Some(detail.into()),
    }
}

pub async fn get_firmware_ota_preflight_snapshot(
    coord: CoordinatorState<'_>,
    _protocol_name: Option<String>,
) -> Result<FirmwareOtaPreflightSnapshot, String> {
    let phase = coord.dictation_phase_for_cli();
    if coord.firmware_ota_transfer_active() {
        return Ok(FirmwareOtaPreflightSnapshot {
            recording_active: phase != SessionPhase::Idle,
            dictation_phase: format!("{phase:?}"),
            device: firmware_ota_active_preflight_snapshot(),
            handoff_elapsed_ms: None,
            target_probe_elapsed_ms: None,
            total_elapsed_ms: 0,
        });
    }
    if phase != SessionPhase::Idle {
        return Ok(FirmwareOtaPreflightSnapshot {
            recording_active: true,
            dictation_phase: format!("{phase:?}"),
            device: firmware_ota_preflight_unavailable_snapshot(
                "Firmware OTA preflight is blocked while dictation is active; stop recording before opening an exclusive OTA GATT session.",
            ),
            handoff_elapsed_ms: None,
            target_probe_elapsed_ms: None,
            total_elapsed_ms: 0,
        });
    }
    let cached_power = coord.embedded_ble_wake_recovery_snapshot();
    let preflight_started = Instant::now();
    /*
     * TYPE:OTA must go out on the live notify capture before try_begin()
     * suppresses dictation (which cancels that capture). The old order was:
     * try_begin → cancel notify → TYPE:OTA write times out 800 ms →
     * connected=false and the UI cannot read firmware info on step 1.
     * Match the real transfer path: handoff while notify is still live first.
     */
    let handoff_started = Instant::now();
    let handoff = crate::embedded_ble::request_listener_ota_v1_active_link(None);
    let handoff_elapsed_ms = elapsed_ms_u64(handoff_started);
    if let Err(err) = &handoff {
        log::warn!(
            "[ota] preflight TYPE:OTA handoff soft-failed; continuing OTA GATT probe: {err}"
        );
    }
    if !coord.try_begin_firmware_ota_transfer() {
        return Ok(FirmwareOtaPreflightSnapshot {
            recording_active: false,
            dictation_phase: format!("{phase:?}"),
            device: firmware_ota_active_preflight_snapshot(),
            handoff_elapsed_ms: Some(handoff_elapsed_ms),
            target_probe_elapsed_ms: None,
            total_elapsed_ms: elapsed_ms_u64(preflight_started),
        });
    }
    let target_probe_started = Instant::now();
    let snapshot_task = tauri::async_runtime::spawn_blocking(move || {
        if handoff.is_ok() {
            crate::embedded_ble::listener_ota_v1_gatt_probe_after_active_link_hint(
                FIRMWARE_OTA_LISTENER_V1_GATT_PROBE_TIMEOUT,
            )
        } else {
            // Soft-fail handoff: still open denzic_ota_v1 so step-1 firmware
            // identity can populate (same soft-fail policy as bulk prepare).
            crate::embedded_ble::listener_ota_v1_gatt_probe_snapshot(
                FIRMWARE_OTA_LISTENER_V1_GATT_PROBE_TIMEOUT,
            )
        }
    });
    let mut device = match tokio::time::timeout(
        FIRMWARE_OTA_LISTENER_V1_PREFLIGHT_TIMEOUT,
        snapshot_task,
    )
    .await
    {
        Ok(joined) => match joined {
            Ok(snapshot) => snapshot,
            Err(err) => firmware_ota_preflight_unavailable_snapshot(format!(
                "Listener BLE OTA preflight task failed: {err}"
            )),
        },
        Err(_) => firmware_ota_preflight_unavailable_snapshot(format!(
            "Listener BLE OTA preflight timed out after {} ms; retry after reconnecting Listener or resetting Windows Bluetooth.",
            FIRMWARE_OTA_LISTENER_V1_PREFLIGHT_TIMEOUT.as_millis()
        )),
    };
    let target_probe_elapsed_ms = Some(elapsed_ms_u64(target_probe_started));
    coord.end_failed_firmware_ota_transfer();
    if device.usb_powered.is_none() {
        device.usb_powered = cached_power.usb_powered;
    }
    if device.battery_percent.is_none() {
        device.battery_percent = cached_power.battery_percent;
    }
    let total_elapsed_ms = elapsed_ms_u64(preflight_started);
    log::info!(
        "[ota] active-link preflight timing handoff_elapsed_ms={handoff_elapsed_ms} target_probe_elapsed_ms={:?} total_elapsed_ms={total_elapsed_ms} connected={}",
        target_probe_elapsed_ms,
        device.connected,
    );
    Ok(FirmwareOtaPreflightSnapshot {
        recording_active: phase != SessionPhase::Idle,
        dictation_phase: format!("{phase:?}"),
        device,
        handoff_elapsed_ms: Some(handoff_elapsed_ms),
        target_probe_elapsed_ms,
        total_elapsed_ms,
    })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FirmwareOtaBleTransferResult {
    pub bytes_transferred: usize,
    pub chunks_sent: usize,
    pub confirmed_version: Option<String>,
    pub transport: &'static str,
    pub pretransfer_type_ready: bool,
    pub pretransfer_type_ready_elapsed_ms: u64,
    pub target_prepare_elapsed_ms: u64,
    pub transfer_elapsed_ms: u64,
    pub confirm_elapsed_ms: u64,
    pub type_ready: bool,
    pub type_ready_elapsed_ms: u64,
    pub non_transfer_fixed_elapsed_ms: u64,
    pub total_elapsed_ms: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FirmwareOtaPackagePayload {
    pub manifest_text: String,
    pub firmware_bytes: Vec<u8>,
    pub source_label: String,
}

pub const FIRMWARE_OTA_CONFIRM_INTERVAL: Duration = Duration::from_secs(2);
pub const FIRMWARE_OTA_CONFIRM_REBOOT_GRACE: Duration = Duration::from_millis(1800);
pub const FIRMWARE_OTA_FAST_SERVICE_CONFIRM_TIMEOUT: Duration = Duration::from_millis(2000);
pub const FIRMWARE_OTA_LISTENER_V1_REACHABLE_CONFIRM_TIMEOUT: Duration = Duration::from_secs(12);
pub const FIRMWARE_OTA_PRETRANSFER_READY_TIMEOUT: Duration = Duration::from_secs(8);
pub const FIRMWARE_OTA_LISTENER_RELEASE_TIMEOUT: Duration = Duration::from_secs(3);
/// The verified post-confirm CCCD reuse path should become stable quickly.
/// If it disconnects immediately, refresh instead of hiding the failure inside
/// the long re-enumeration window.
pub const FIRMWARE_OTA_POST_READY_FAST_PATH_TIMEOUT: Duration = Duration::from_millis(3000);
pub const FIRMWARE_OTA_HANDOFF_RETRY_TIMEOUT: Duration = Duration::from_secs(3);
/// Post-OTA Windows BLE re-enumeration often exceeds 8s (reboot + radio settle).
/// Keep a longer first window so Type can reattach without a manual re-pair click.
pub const FIRMWARE_OTA_POST_READY_TIMEOUT: Duration = Duration::from_secs(28);
/// Second chance after a forced listener refresh when the first window times out.
pub const FIRMWARE_OTA_POST_READY_RETRY_TIMEOUT: Duration = Duration::from_secs(20);
/// After OTA service is reachable, wait before notify/CCCD so reboot handoff and
/// Windows radio settle finish. Opening CCCD too early races disconnect (25s CCCD
/// timeouts, then TYPE:READY drops — owner must restart Type).
pub const FIRMWARE_OTA_TARGET_PREPARE_TIMEOUT: Duration = Duration::from_secs(6);
pub const FIRMWARE_OTA_PACKAGE_MAX_BYTES: u64 = 16 * 1024 * 1024;

pub fn elapsed_ms_u64(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

pub fn normalize_firmware_ota_version(value: &str) -> String {
    value.trim().trim_start_matches('v').to_ascii_lowercase()
}

pub fn firmware_ota_versions_match(confirmed: &str, expected: &str) -> bool {
    let confirmed = normalize_firmware_ota_version(confirmed);
    let expected = normalize_firmware_ota_version(expected);
    !confirmed.is_empty() && !expected.is_empty() && confirmed == expected
}

pub fn firmware_ota_snapshot_version(
    snapshot: &crate::embedded_ble::FirmwareOtaDeviceSnapshot,
) -> Option<String> {
    snapshot
        .firmware_version
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

pub fn firmware_ota_active_preflight_snapshot() -> crate::embedded_ble::FirmwareOtaDeviceSnapshot {
    crate::embedded_ble::FirmwareOtaDeviceSnapshot {
        connected: true,
        hardware_revision: None,
        firmware_version: None,
        capabilities: vec!["firmware_ota_transfer_active".to_string()],
        battery_percent: None,
        usb_powered: None,
        detail: Some(
            "Firmware OTA is in progress; preflight GATT snapshot is paused to keep the BLE link exclusive."
                .to_string(),
        ),
    }
}

#[derive(Debug, Clone)]
pub struct FirmwareOtaConfirmOutcome {
    pub confirmed_version: Option<String>,
    pub elapsed_ms: u64,
    pub attempts: usize,
    pub matched: bool,
}

pub async fn confirm_listener_ota_v1_reachable(
    expected_version: &str,
) -> FirmwareOtaConfirmOutcome {
    let started = Instant::now();
    if normalize_firmware_ota_version(expected_version).is_empty() || expected_version == "unknown"
    {
        return FirmwareOtaConfirmOutcome {
            confirmed_version: None,
            elapsed_ms: elapsed_ms_u64(started),
            attempts: 0,
            matched: false,
        };
    }

    let reboot_att_ready = crate::embedded_ble::take_listener_ota_v1_reboot_att_ready();
    let reboot_generation_connected =
        crate::embedded_ble::take_listener_ota_v1_reboot_generation_connected();
    if reboot_att_ready {
        log::info!(
            "[firmware-ota] complete post-disconnect device generation retained; final response-bearing TYPE:READY remains required"
        );
        return FirmwareOtaConfirmOutcome {
            confirmed_version: Some(expected_version.to_string()),
            elapsed_ms: elapsed_ms_u64(started),
            attempts: 1,
            matched: true,
        };
    }
    if reboot_generation_connected {
        log::info!(
            "[firmware-ota] new OTA device generation retained after reconnect; starting uncached ATT service confirmation without fixed reboot grace"
        );
    } else {
        tokio::time::sleep(FIRMWARE_OTA_CONFIRM_REBOOT_GRACE).await;
    }

    let deadline = Instant::now() + FIRMWARE_OTA_LISTENER_V1_REACHABLE_CONFIRM_TIMEOUT;
    let mut attempts = 0usize;

    loop {
        attempts += 1;
        let remaining = deadline.saturating_duration_since(Instant::now());
        let fast_timeout = remaining.min(FIRMWARE_OTA_FAST_SERVICE_CONFIRM_TIMEOUT);
        let fast_snapshot = if fast_timeout.is_zero() {
            None
        } else {
            tauri::async_runtime::spawn_blocking(move || {
                crate::embedded_ble::listener_ota_v1_service_reachable_snapshot(fast_timeout)
            })
            .await
            .ok()
        };
        let snapshot = match fast_snapshot {
            Some(snapshot)
                if snapshot.connected
                    && snapshot.capabilities.iter().any(|item| {
                        item == crate::firmware_ota::LISTENER_OTA_V1_FIRMWARE_CAPABILITY
                    }) =>
            {
                Some(snapshot)
            }
            Some(snapshot) => {
                log::info!(
                    "[firmware-ota] fast OTA service confirmation did not reach the Listener; falling back to complete GATT probe detail={:?}",
                    snapshot.detail
                );
                tauri::async_runtime::spawn_blocking(|| {
                    crate::embedded_ble::listener_ota_v1_gatt_probe_snapshot(
                        FIRMWARE_OTA_LISTENER_V1_REACHABLE_CONFIRM_TIMEOUT,
                    )
                })
                .await
                .ok()
            }
            None => None,
        };
        if let Some(snapshot) = snapshot {
            let reachable = snapshot.connected
                && snapshot
                    .capabilities
                    .iter()
                    .any(|item| item == crate::firmware_ota::LISTENER_OTA_V1_FIRMWARE_CAPABILITY);
            if reachable {
                let outcome = FirmwareOtaConfirmOutcome {
                    confirmed_version: Some(expected_version.to_string()),
                    elapsed_ms: elapsed_ms_u64(started),
                    attempts,
                    matched: true,
                };
                log::info!(
                    "[firmware-ota] Listener OTA v1 service reachable after transfer expected={expected_version} attempts={} elapsed_ms={}",
                    outcome.attempts,
                    outcome.elapsed_ms
                );
                return outcome;
            }
        }

        if Instant::now() >= deadline {
            let outcome = FirmwareOtaConfirmOutcome {
                confirmed_version: None,
                elapsed_ms: elapsed_ms_u64(started),
                attempts,
                matched: false,
            };
            log::warn!(
                "[firmware-ota] Listener OTA v1 reachable confirmation timed out expected={expected_version} attempts={} elapsed_ms={}",
                outcome.attempts,
                outcome.elapsed_ms
            );
            return outcome;
        }
        tokio::time::sleep(FIRMWARE_OTA_CONFIRM_INTERVAL).await;
    }
}

pub fn load_firmware_ota_package(path: String) -> Result<FirmwareOtaPackagePayload, String> {
    let path = PathBuf::from(path);
    if path.is_dir() {
        return load_firmware_ota_package_dir(&path);
    }
    let is_zip = path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.eq_ignore_ascii_case("zip"))
        .unwrap_or(false);
    if is_zip {
        return load_firmware_ota_package_zip(&path);
    }
    Err("OTA package must be a .zip file or a package directory.".to_string())
}

pub fn load_firmware_ota_package_dir(path: &Path) -> Result<FirmwareOtaPackagePayload, String> {
    let manifest_path = path.join("ota_manifest.json");
    let firmware_path = path.join("firmware_ota.bin");
    let manifest_text = std::fs::read_to_string(&manifest_path)
        .map_err(|err| format!("Failed to read {}: {err}", manifest_path.display()))?;
    let firmware_bytes = read_limited_file(&firmware_path)?;
    Ok(FirmwareOtaPackagePayload {
        manifest_text,
        firmware_bytes,
        source_label: path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("OTA package directory")
            .to_string(),
    })
}

pub fn load_firmware_ota_package_zip(path: &Path) -> Result<FirmwareOtaPackagePayload, String> {
    let file =
        File::open(path).map_err(|err| format!("Failed to open {}: {err}", path.display()))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|err| format!("Invalid OTA zip package: {err}"))?;
    let manifest_text =
        read_zip_entry_by_basename(&mut archive, "ota_manifest.json").and_then(|bytes| {
            String::from_utf8(bytes)
                .map_err(|err| format!("ota_manifest.json is not valid UTF-8: {err}"))
        })?;
    let firmware_bytes = read_zip_entry_by_basename(&mut archive, "firmware_ota.bin")?;
    Ok(FirmwareOtaPackagePayload {
        manifest_text,
        firmware_bytes,
        source_label: path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("OTA zip package")
            .to_string(),
    })
}

pub fn read_limited_file(path: &Path) -> Result<Vec<u8>, String> {
    let metadata = std::fs::metadata(path)
        .map_err(|err| format!("Failed to stat {}: {err}", path.display()))?;
    if metadata.len() > FIRMWARE_OTA_PACKAGE_MAX_BYTES {
        return Err(format!(
            "{} is too large for an OTA package ({} bytes > {} bytes).",
            path.display(),
            metadata.len(),
            FIRMWARE_OTA_PACKAGE_MAX_BYTES
        ));
    }
    std::fs::read(path).map_err(|err| format!("Failed to read {}: {err}", path.display()))
}

pub fn read_zip_entry_by_basename<R: std::io::Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    basename: &str,
) -> Result<Vec<u8>, String> {
    let mut match_index = None;
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|err| format!("Failed to read OTA zip entry #{index}: {err}"))?;
        if entry.is_dir() {
            continue;
        }
        let normalized = entry.name().replace('\\', "/");
        if normalized
            .rsplit('/')
            .next()
            .map(|name| name == basename)
            .unwrap_or(false)
        {
            match_index = Some(index);
            break;
        }
    }

    let index = match_index.ok_or_else(|| format!("OTA zip is missing {basename}."))?;
    let mut entry = archive
        .by_index(index)
        .map_err(|err| format!("Failed to open OTA zip entry {basename}: {err}"))?;
    if entry.size() > FIRMWARE_OTA_PACKAGE_MAX_BYTES {
        return Err(format!(
            "{basename} is too large for an OTA package ({} bytes > {} bytes).",
            entry.size(),
            FIRMWARE_OTA_PACKAGE_MAX_BYTES
        ));
    }
    let mut bytes = Vec::with_capacity(entry.size().min(usize::MAX as u64) as usize);
    entry
        .read_to_end(&mut bytes)
        .map_err(|err| format!("Failed to read OTA zip entry {basename}: {err}"))?;
    Ok(bytes)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WiredFirmwarePackageKind {
    Factory,
}

impl WiredFirmwarePackageKind {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Factory => "factory",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WiredFirmwareTargetKind {
    Esp32S3,
}

impl WiredFirmwareTargetKind {
    fn supports_boot_repair(self) -> bool {
        matches!(self, Self::Esp32S3)
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct FactoryFirmwareManifest {
    pub schema_version: u64,
    pub project: String,
    pub version: String,
    pub target: String,
    #[serde(default)]
    pub git_commit: String,
    #[serde(default)]
    pub flash: Option<FactoryFirmwareFlash>,
    pub artifacts: Vec<FactoryFirmwareArtifact>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct FactoryFirmwareFlash {
    #[serde(default)]
    pub tool: Option<String>,
    #[serde(default)]
    pub port: Option<String>,
    #[serde(default)]
    pub baud: Option<Value>,
    #[serde(default)]
    pub partition_table: Vec<FactoryFirmwarePartition>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct FactoryFirmwarePartition {
    pub name: String,
    pub offset: String,
    pub size: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct FactoryFirmwareArtifact {
    pub role: String,
    pub file: String,
    pub offset: String,
    #[serde(default)]
    pub format: Option<String>,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WiredFirmwareArtifactInfo {
    pub role: String,
    pub file: String,
    pub offset: String,
    pub size_bytes: u64,
    pub sha256: String,
}

impl From<&FactoryFirmwareArtifact> for WiredFirmwareArtifactInfo {
    fn from(value: &FactoryFirmwareArtifact) -> Self {
        Self {
            role: value.role.clone(),
            file: value.file.clone(),
            offset: value.offset.clone(),
            size_bytes: value.size_bytes,
            sha256: value.sha256.clone(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WiredFirmwarePackagePayload {
    pub kind: String,
    pub project: String,
    pub version: String,
    pub target: String,
    pub git_commit: Option<String>,
    pub source_label: String,
    pub artifacts: Vec<WiredFirmwareArtifactInfo>,
    pub supports_full_flash: bool,
    pub supports_boot_repair: bool,
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WiredFirmwareSerialPort {
    pub port: String,
    pub label: String,
    pub is_likely_esp32: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WiredFirmwareFlashResult {
    pub action: String,
    pub kind: String,
    pub port: String,
    pub version: String,
    pub log: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WiredFirmwareProgressPayload {
    pub action: String,
    pub stage: String,
    pub port: Option<String>,
    pub version: Option<String>,
    pub current_role: Option<String>,
    pub current_file: Option<String>,
    pub bytes_written: u64,
    pub bytes_total: u64,
    pub current_bytes: u64,
    pub current_total: u64,
    pub percent: u8,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct LoadedWiredFirmwarePackage {
    pub kind: WiredFirmwarePackageKind,
    pub project: String,
    pub version: String,
    pub target: String,
    pub git_commit: Option<String>,
    pub source_label: String,
    pub artifacts: Vec<FactoryFirmwareArtifact>,
    pub files: BTreeMap<String, Vec<u8>>,
    pub manifest_file_name: &'static str,
    pub manifest_text: String,
    pub otadata_region: Option<(String, String)>,
    pub notes: Vec<String>,
}

impl LoadedWiredFirmwarePackage {
    pub fn to_payload(&self) -> WiredFirmwarePackagePayload {
        let target_kind = wired_firmware_target_kind(&self.target).ok();
        WiredFirmwarePackagePayload {
            kind: self.kind.as_str().to_string(),
            project: self.project.clone(),
            version: self.version.clone(),
            target: self.target.clone(),
            git_commit: self.git_commit.clone(),
            source_label: self.source_label.clone(),
            artifacts: self
                .artifacts
                .iter()
                .map(WiredFirmwareArtifactInfo::from)
                .collect(),
            supports_full_flash: true,
            supports_boot_repair: target_kind
                .map(WiredFirmwareTargetKind::supports_boot_repair)
                .unwrap_or(false),
            notes: self.notes.clone(),
        }
    }
}

pub const WIRED_FIRMWARE_PACKAGE_MAX_BYTES: u64 = 64 * 1024 * 1024;
pub const WIRED_OTADATA_OFFSET: &str = "0xf000";
pub const WIRED_OTADATA_SIZE: &str = "0x2000";
pub const WIRED_DEFAULT_BAUD: u32 = 921_600;
pub const WIRED_BOOT_REPAIR_BAUDS: &[u32] = &[115_200, 57_600, 9_600];
pub const WIRED_FLASH_MODE: FlashMode = FlashMode::Dio;
pub const WIRED_FLASH_FREQUENCY: FlashFrequency = FlashFrequency::_80Mhz;
pub const WIRED_FLASH_SIZE: FlashSize = FlashSize::_16Mb;
pub const WIRED_FULL_FLASH_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
pub const WIRED_BOOT_REPAIR_CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
pub const WIRED_FLASH_CONNECT_RETRY_INTERVAL: Duration = Duration::from_millis(120);
pub const WIRED_FLASH_SERIAL_IO_TIMEOUT: Duration = Duration::from_secs(5);

pub struct PreparedWiredFlashArtifact {
    pub role: String,
    pub file: String,
    pub offset: u32,
    pub bytes: Vec<u8>,
    pub patch_report: Option<EspImagePatchReport>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct EspImagePatchReport {
    pub changed: bool,
    pub digest_recalculated: bool,
}

#[derive(Clone, Debug)]
pub struct WiredFlashProgressArtifact {
    pub role: String,
    pub file: String,
    pub offset: u32,
    pub size_bytes: u64,
}

#[derive(Clone, Debug)]
pub struct WiredFlashProgressCurrent {
    pub artifact: WiredFlashProgressArtifact,
    pub total_units: usize,
    pub current_units: usize,
}

pub struct WiredFlashProgressCallbacks {
    pub app: Option<AppHandle>,
    pub action: String,
    pub version: String,
    pub port: String,
    pub artifacts: Vec<WiredFlashProgressArtifact>,
    pub total_bytes: u64,
    pub completed_bytes: u64,
    pub current: Option<WiredFlashProgressCurrent>,
    pub stage_start_percent: u8,
    pub stage_end_percent: u8,
    pub last_emitted_percent: Option<u8>,
    pub last_emitted_bytes: u64,
}

impl WiredFlashProgressCallbacks {
    fn new(
        app: Option<&AppHandle>,
        action: &str,
        version: &str,
        port: &str,
        artifacts: Vec<WiredFlashProgressArtifact>,
        stage_start_percent: u8,
        stage_end_percent: u8,
    ) -> Self {
        let total_bytes = artifacts
            .iter()
            .map(|artifact| artifact.size_bytes)
            .sum::<u64>()
            .max(1);
        Self {
            app: app.cloned(),
            action: action.to_string(),
            version: version.to_string(),
            port: port.to_string(),
            artifacts,
            total_bytes,
            completed_bytes: 0,
            current: None,
            stage_start_percent,
            stage_end_percent: stage_end_percent.max(stage_start_percent),
            last_emitted_percent: None,
            last_emitted_bytes: 0,
        }
    }

    fn emit_current(&mut self, force: bool) {
        let Some(current) = self.current.as_ref() else {
            return;
        };
        let current_bytes = scaled_progress_bytes(
            current.artifact.size_bytes,
            current.current_units,
            current.total_units,
        );
        let bytes_written = self
            .completed_bytes
            .saturating_add(current_bytes)
            .min(self.total_bytes);
        let percent = wired_progress_percent_for_range(
            bytes_written,
            self.total_bytes,
            self.stage_start_percent,
            self.stage_end_percent,
        );
        if !force
            && self.last_emitted_percent == Some(percent)
            && bytes_written.saturating_sub(self.last_emitted_bytes) < 32 * 1024
        {
            return;
        }
        self.last_emitted_percent = Some(percent);
        self.last_emitted_bytes = bytes_written;
        emit_wired_firmware_progress(
            self.app.as_ref(),
            WiredFirmwareProgressPayload {
                action: self.action.clone(),
                stage: "writing".to_string(),
                port: Some(self.port.clone()),
                version: Some(self.version.clone()),
                current_role: Some(current.artifact.role.clone()),
                current_file: Some(current.artifact.file.clone()),
                bytes_written,
                bytes_total: self.total_bytes,
                current_bytes,
                current_total: current.artifact.size_bytes,
                percent,
                message: format!("Writing {}", current.artifact.file),
            },
        );
    }
}

impl ProgressCallbacks for WiredFlashProgressCallbacks {
    fn init(&mut self, addr: u32, total: usize) {
        let artifact = self
            .artifacts
            .iter()
            .find(|artifact| artifact.offset == addr)
            .cloned()
            .unwrap_or_else(|| WiredFlashProgressArtifact {
                role: "firmware".to_string(),
                file: format!("0x{addr:x}"),
                offset: addr,
                size_bytes: 0,
            });
        self.current = Some(WiredFlashProgressCurrent {
            artifact,
            total_units: total.max(1),
            current_units: 0,
        });
        self.emit_current(true);
    }

    fn update(&mut self, current: usize) {
        if let Some(progress) = self.current.as_mut() {
            progress.current_units = current.min(progress.total_units);
        }
        self.emit_current(false);
    }

    fn finish(&mut self) {
        if let Some(progress) = self.current.as_mut() {
            progress.current_units = progress.total_units;
        }
        self.emit_current(true);
        if let Some(progress) = self.current.take() {
            self.completed_bytes = self
                .completed_bytes
                .saturating_add(progress.artifact.size_bytes)
                .min(self.total_bytes);
        }
    }
}

pub fn emit_wired_firmware_progress(
    app: Option<&AppHandle>,
    payload: WiredFirmwareProgressPayload,
) {
    if let Some(app) = app {
        let _ = app.emit("wired-firmware:progress", payload);
    }
}

pub fn emit_wired_firmware_stage(
    app: Option<&AppHandle>,
    action: &str,
    stage: &str,
    version: Option<&str>,
    port: Option<&str>,
    percent: u8,
    message: impl Into<String>,
) {
    emit_wired_firmware_progress(
        app,
        WiredFirmwareProgressPayload {
            action: action.to_string(),
            stage: stage.to_string(),
            port: port.map(ToOwned::to_owned),
            version: version.map(ToOwned::to_owned),
            current_role: None,
            current_file: None,
            bytes_written: 0,
            bytes_total: 0,
            current_bytes: 0,
            current_total: 0,
            percent: percent.min(100),
            message: message.into(),
        },
    );
}

pub fn scaled_progress_bytes(total_bytes: u64, current_units: usize, total_units: usize) -> u64 {
    if total_units == 0 {
        return total_bytes;
    }
    ((total_bytes as u128)
        .saturating_mul(current_units as u128)
        .checked_div(total_units as u128)
        .unwrap_or(0)
        .min(total_bytes as u128)) as u64
}

pub fn wired_progress_percent_for_range(
    bytes_written: u64,
    bytes_total: u64,
    start_percent: u8,
    end_percent: u8,
) -> u8 {
    if bytes_total == 0 {
        return end_percent.min(100);
    }
    let span = u16::from(end_percent.saturating_sub(start_percent));
    let delta = ((bytes_written as u128)
        .saturating_mul(span as u128)
        .checked_div(bytes_total as u128)
        .unwrap_or(0)
        .min(span as u128)) as u8;
    start_percent.saturating_add(delta).min(100)
}

pub fn list_wired_firmware_ports() -> Result<Vec<WiredFirmwareSerialPort>, String> {
    Ok(list_wired_firmware_ports_internal())
}

pub fn load_wired_firmware_package(path: String) -> Result<WiredFirmwarePackagePayload, String> {
    load_wired_firmware_package_internal(&PathBuf::from(path)).map(|loaded| loaded.to_payload())
}

pub async fn flash_wired_firmware_package(
    app: AppHandle,
    path: String,
    port: Option<String>,
    baud: Option<u32>,
    preserve_ota_data: Option<bool>,
) -> Result<WiredFirmwareFlashResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        run_wired_firmware_flash_with_progress(
            &PathBuf::from(path),
            port.as_deref(),
            baud,
            preserve_ota_data.unwrap_or(false),
            Some(app),
        )
    })
    .await
    .map_err(|err| format!("Wired firmware flash task failed: {err}"))?
}

pub async fn repair_wired_firmware_bootloader(
    app: AppHandle,
    path: String,
    port: Option<String>,
    baud: Option<u32>,
) -> Result<WiredFirmwareFlashResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        run_wired_bootloader_repair_with_progress(
            &PathBuf::from(path),
            port.as_deref(),
            baud,
            Some(app),
        )
    })
    .await
    .map_err(|err| format!("Wired bootloader repair task failed: {err}"))?
}

pub fn list_wired_firmware_ports_internal() -> Vec<WiredFirmwareSerialPort> {
    let mut ports = serialport::available_ports().unwrap_or_default();
    ports.sort_by(|a, b| a.port_name.cmp(&b.port_name));
    ports
        .into_iter()
        .map(|port| {
            let mut label = port.port_name.clone();
            let mut is_likely_esp32 = false;
            if let serialport::SerialPortType::UsbPort(info) = port.port_type {
                is_likely_esp32 = info.vid == 0x303a || info.pid == 0x1001;
                let details = [info.manufacturer, info.product]
                    .into_iter()
                    .flatten()
                    .filter(|value| !value.trim().is_empty())
                    .collect::<Vec<_>>()
                    .join(" ");
                if !details.is_empty() {
                    label = format!("{} · {}", port.port_name, details);
                }
            }
            WiredFirmwareSerialPort {
                port: port.port_name,
                label,
                is_likely_esp32,
            }
        })
        .collect()
}

pub fn resolve_wired_flash_port(requested: Option<&str>) -> Result<String, String> {
    let requested = requested.map(str::trim).filter(|value| !value.is_empty());
    if let Some(value) = requested {
        if !value.eq_ignore_ascii_case("COMx") {
            return Ok(value.to_string());
        }
    }

    let ports = list_wired_firmware_ports_internal();
    if ports.is_empty() {
        return Err(
            "No serial ports were detected. Connect the Listener ESP32-S3 USB port and retry."
                .to_string(),
        );
    }
    let esp32_ports = ports
        .iter()
        .filter(|port| port.is_likely_esp32)
        .collect::<Vec<_>>();
    if esp32_ports.len() == 1 {
        return Ok(esp32_ports[0].port.clone());
    }
    if ports.len() == 1 {
        return Ok(ports[0].port.clone());
    }

    let summary = ports
        .iter()
        .map(|port| port.label.clone())
        .collect::<Vec<_>>()
        .join("; ");
    Err(format!(
        "Multiple serial ports were detected. Choose the Listener COM port explicitly. Ports: {summary}"
    ))
}

pub fn load_wired_firmware_package_internal(
    path: &Path,
) -> Result<LoadedWiredFirmwarePackage, String> {
    if path.is_dir() {
        if path.join("manifest.json").is_file() {
            return load_factory_firmware_package_dir(path);
        }
        if let Some(factory_dir) = find_factory_package_dir(path) {
            return load_factory_firmware_package_dir(&factory_dir);
        }
        return Err(format!(
            "Wired firmware flashing requires a factory package directory with manifest.json, or a directory containing factory/<package>/manifest.json: {}",
            path.display()
        ));
    }

    let is_zip = path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.eq_ignore_ascii_case("zip"))
        .unwrap_or(false);
    if !is_zip {
        return Err("Wired firmware package must be a .zip file or package directory.".to_string());
    }

    if zip_entry_exists(path, "manifest.json")? {
        return load_factory_firmware_package_zip(path);
    }
    if zip_entry_exists(path, "ota_manifest.json")? {
        return Err("Wired firmware flashing uses the factory package format, not a Bluetooth OTA zip. Select a factory zip or package directory containing manifest.json.".to_string());
    }
    Err("Wired firmware zip is missing factory manifest.json.".to_string())
}

pub fn find_factory_package_dir(path: &Path) -> Option<PathBuf> {
    if path.join("manifest.json").is_file() {
        return Some(path.to_path_buf());
    }

    let factory_root = path.join("factory");
    let entries = std::fs::read_dir(factory_root).ok()?;
    let mut candidates = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|candidate| candidate.join("manifest.json").is_file())
        .collect::<Vec<_>>();
    candidates.sort();
    candidates.into_iter().next()
}

pub fn source_label_for_path(path: &Path, fallback: &str) -> String {
    path.file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(fallback)
        .to_string()
}

pub fn load_factory_firmware_package_dir(
    path: &Path,
) -> Result<LoadedWiredFirmwarePackage, String> {
    let manifest_path = path.join("manifest.json");
    let manifest_text = std::fs::read_to_string(&manifest_path)
        .map_err(|err| format!("Failed to read {}: {err}", manifest_path.display()))?;
    let manifest = parse_factory_firmware_manifest(&manifest_text)?;
    let mut files = BTreeMap::new();
    for artifact in &manifest.artifacts {
        validate_factory_artifact(artifact)?;
        let bytes = read_limited_wired_file(&path.join(&artifact.file))?;
        validate_factory_artifact_bytes(artifact, &bytes)?;
        files.insert(artifact.file.clone(), bytes);
    }
    loaded_factory_package_from_manifest(
        manifest,
        manifest_text,
        source_label_for_path(path, "factory firmware package"),
        files,
    )
}

pub fn load_factory_firmware_package_zip(
    path: &Path,
) -> Result<LoadedWiredFirmwarePackage, String> {
    let manifest_bytes = read_zip_entry_by_basename_limited(path, "manifest.json")?
        .ok_or_else(|| "Factory firmware zip is missing manifest.json.".to_string())?;
    let manifest_text = String::from_utf8(manifest_bytes)
        .map_err(|err| format!("manifest.json is not valid UTF-8: {err}"))?;
    let manifest = parse_factory_firmware_manifest(&manifest_text)?;
    let mut files = BTreeMap::new();
    for artifact in &manifest.artifacts {
        validate_factory_artifact(artifact)?;
        let bytes = read_zip_entry_by_basename_limited(path, &artifact.file)?
            .ok_or_else(|| format!("Factory firmware zip is missing {}.", artifact.file))?;
        validate_factory_artifact_bytes(artifact, &bytes)?;
        files.insert(artifact.file.clone(), bytes);
    }
    loaded_factory_package_from_manifest(
        manifest,
        manifest_text,
        source_label_for_path(path, "factory firmware zip"),
        files,
    )
}

pub fn parse_factory_firmware_manifest(text: &str) -> Result<FactoryFirmwareManifest, String> {
    let manifest: FactoryFirmwareManifest =
        serde_json::from_str(text).map_err(|err| format!("manifest.json is invalid: {err}"))?;
    if manifest.schema_version != 1 {
        return Err(format!(
            "Factory manifest schema_version must be 1, got {}.",
            manifest.schema_version
        ));
    }
    let target_kind = wired_firmware_target_kind(&manifest.target)?;
    match target_kind {
        WiredFirmwareTargetKind::Esp32S3 if manifest.project != "voice-keyboard-firmware" => {
            return Err(format!(
                "Factory manifest project must be voice-keyboard-firmware for ESP32-S3, got {}.",
                manifest.project
            ));
        }
        _ => {}
    }
    Ok(manifest)
}

pub fn loaded_factory_package_from_manifest(
    manifest: FactoryFirmwareManifest,
    manifest_text: String,
    source_label: String,
    files: BTreeMap<String, Vec<u8>>,
) -> Result<LoadedWiredFirmwarePackage, String> {
    let target_kind = wired_firmware_target_kind(&manifest.target)?;
    let (otadata_region, notes) = match target_kind {
        WiredFirmwareTargetKind::Esp32S3 => {
            for role in ["bootloader", "partition_table", "app"] {
                require_artifact(&manifest.artifacts, role)?;
            }
            let otadata_region = manifest.flash.as_ref().and_then(|flash| {
                flash
                    .partition_table
                    .iter()
                    .find(|entry| entry.name == "otadata")
            });
            let otadata_region = match otadata_region {
                Some(entry) => Some((
                    normalize_esptool_region_arg(&entry.offset, "otadata offset", true)?,
                    normalize_esptool_region_arg(&entry.size, "otadata size", false)?,
                )),
                None => Some((
                    WIRED_OTADATA_OFFSET.to_string(),
                    WIRED_OTADATA_SIZE.to_string(),
                )),
            };
            (
                otadata_region,
                vec![
                    "Factory package: wired flash auto-checks boot at 0x0, then writes bootloader, partition table, and app."
                        .to_string(),
                    "No separate Boot repair button is required; missing/corrupt boot is repaired as part of 有线刷入."
                        .to_string(),
                ],
            )
        }
    };
    Ok(LoadedWiredFirmwarePackage {
        kind: WiredFirmwarePackageKind::Factory,
        project: manifest.project,
        version: manifest.version,
        target: manifest.target,
        git_commit: (!manifest.git_commit.trim().is_empty()).then_some(manifest.git_commit),
        source_label,
        artifacts: manifest.artifacts,
        files,
        manifest_file_name: "manifest.json",
        manifest_text,
        otadata_region,
        notes,
    })
}

pub fn validate_factory_artifact(artifact: &FactoryFirmwareArtifact) -> Result<(), String> {
    if artifact.role.trim().is_empty() {
        return Err("Factory artifact role must not be empty.".to_string());
    }
    if artifact.offset.trim().is_empty() {
        return Err(format!(
            "Factory artifact {} offset is empty.",
            artifact.role
        ));
    }
    validate_package_file_name(&artifact.file)?;
    if artifact.size_bytes == 0 {
        return Err(format!("Factory artifact {} is empty.", artifact.file));
    }
    if !is_sha256_hex(&artifact.sha256) {
        return Err(format!(
            "Factory artifact {} has invalid SHA256.",
            artifact.file
        ));
    }
    Ok(())
}

pub fn validate_package_file_name(file_name: &str) -> Result<(), String> {
    if file_name.trim().is_empty()
        || file_name.contains('/')
        || file_name.contains('\\')
        || file_name.contains("..")
        || file_name.chars().any(char::is_control)
    {
        return Err(format!("Unsafe firmware package file name: {file_name}"));
    }
    Ok(())
}

pub fn validate_factory_artifact_bytes(
    artifact: &FactoryFirmwareArtifact,
    bytes: &[u8],
) -> Result<(), String> {
    if bytes.len() as u64 != artifact.size_bytes {
        return Err(format!(
            "{} size mismatch: manifest={} actual={}.",
            artifact.file,
            artifact.size_bytes,
            bytes.len()
        ));
    }
    let actual_sha256 = crate::firmware_ota::sha256_hex(bytes);
    if actual_sha256 != artifact.sha256.to_ascii_lowercase() {
        return Err(format!(
            "{} SHA256 does not match manifest.json.",
            artifact.file
        ));
    }
    Ok(())
}

pub fn require_artifact<'a>(
    artifacts: &'a [FactoryFirmwareArtifact],
    role: &str,
) -> Result<&'a FactoryFirmwareArtifact, String> {
    artifacts
        .iter()
        .find(|artifact| artifact.role == role)
        .ok_or_else(|| format!("Firmware package is missing {role} artifact."))
}

pub fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.chars().all(|ch| ch.is_ascii_hexdigit())
}

pub fn read_limited_wired_file(path: &Path) -> Result<Vec<u8>, String> {
    let metadata = std::fs::metadata(path)
        .map_err(|err| format!("Failed to stat {}: {err}", path.display()))?;
    if metadata.len() > WIRED_FIRMWARE_PACKAGE_MAX_BYTES {
        return Err(format!(
            "{} is too large for a firmware package ({} bytes > {} bytes).",
            path.display(),
            metadata.len(),
            WIRED_FIRMWARE_PACKAGE_MAX_BYTES
        ));
    }
    std::fs::read(path).map_err(|err| format!("Failed to read {}: {err}", path.display()))
}

pub fn zip_entry_exists(path: &Path, basename: &str) -> Result<bool, String> {
    read_zip_entry_by_basename_limited(path, basename).map(|entry| entry.is_some())
}

pub fn read_zip_entry_by_basename_limited(
    path: &Path,
    basename: &str,
) -> Result<Option<Vec<u8>>, String> {
    let file =
        File::open(path).map_err(|err| format!("Failed to open {}: {err}", path.display()))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|err| format!("Invalid firmware zip package: {err}"))?;
    let mut match_index = None;
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|err| format!("Failed to read firmware zip entry #{index}: {err}"))?;
        if entry.is_dir() {
            continue;
        }
        let normalized = entry.name().replace('\\', "/");
        if normalized
            .rsplit('/')
            .next()
            .map(|name| name == basename)
            .unwrap_or(false)
        {
            match_index = Some(index);
            break;
        }
    }

    let Some(index) = match_index else {
        return Ok(None);
    };
    let mut entry = archive
        .by_index(index)
        .map_err(|err| format!("Failed to open firmware zip entry {basename}: {err}"))?;
    if entry.size() > WIRED_FIRMWARE_PACKAGE_MAX_BYTES {
        return Err(format!(
            "{basename} is too large for a firmware package ({} bytes > {} bytes).",
            entry.size(),
            WIRED_FIRMWARE_PACKAGE_MAX_BYTES
        ));
    }
    let mut bytes = Vec::with_capacity(entry.size().min(usize::MAX as u64) as usize);
    entry
        .read_to_end(&mut bytes)
        .map_err(|err| format!("Failed to read firmware zip entry {basename}: {err}"))?;
    Ok(Some(bytes))
}

pub fn run_wired_firmware_flash(
    path: &Path,
    requested_port: Option<&str>,
    baud: Option<u32>,
    preserve_ota_data: bool,
) -> Result<WiredFirmwareFlashResult, String> {
    run_wired_firmware_flash_with_progress(path, requested_port, baud, preserve_ota_data, None)
}

pub fn run_wired_firmware_flash_with_progress(
    path: &Path,
    requested_port: Option<&str>,
    baud: Option<u32>,
    preserve_ota_data: bool,
    progress_app: Option<AppHandle>,
) -> Result<WiredFirmwareFlashResult, String> {
    let progress_app_ref = progress_app.as_ref();
    emit_wired_firmware_stage(
        progress_app_ref,
        "flash",
        "loading",
        None,
        requested_port,
        1,
        "Loading wired firmware package",
    );
    let loaded = load_wired_firmware_package_internal(path)?;
    emit_wired_firmware_stage(
        progress_app_ref,
        "flash",
        "packageLoaded",
        Some(&loaded.version),
        requested_port,
        4,
        "Wired firmware package loaded",
    );

    let chip = wired_target_chip(&loaded.target)?;
    let port_hint = match normalize_requested_wired_port(requested_port) {
        Some(port) => port,
        None => resolve_wired_flash_port(None)?,
    };
    let baud = baud
        .or_else(|| package_manifest_baud(&loaded))
        .unwrap_or(WIRED_DEFAULT_BAUD);
    let mut log = String::new();
    log.push_str("> Listener Type built-in wired firmware flasher\n");
    log.push_str("No ESP-IDF, IDF_PATH, Python, or esptool.py environment is required.\n");
    log.push_str(&format!(
        "Package: {} ({})\nTarget: {}\nBaud: {}\n",
        loaded.source_label, loaded.version, loaded.target, baud
    ));

    emit_wired_firmware_stage(
        progress_app_ref,
        "flash",
        "connecting",
        Some(&loaded.version),
        Some(&port_hint),
        8,
        "Connecting to ESP32-S3 serial flasher",
    );
    let (mut flasher, port, connect_log) = connect_builtin_esp_flasher_with_wait(
        Some(&port_hint),
        baud,
        chip,
        true,
        true,
        ResetBeforeOperation::DefaultReset,
        ResetAfterOperation::HardReset,
        WIRED_FULL_FLASH_CONNECT_TIMEOUT,
        true,
    )?;
    emit_wired_firmware_stage(
        progress_app_ref,
        "flash",
        "connected",
        Some(&loaded.version),
        Some(&port),
        16,
        "Connected to ESP32-S3 flasher",
    );
    log.push_str(&connect_log);
    flasher.set_flash_size(WIRED_FLASH_SIZE);
    match flasher.device_info() {
        Ok(info) => log.push_str(&format_wired_device_info(&info)),
        Err(err) => log.push_str(&format!("Device info read skipped: {err}\n")),
    }
    log.push_str(&format!(
        "Flash config: mode={:?}, frequency={:?}, size={:?}\n",
        WIRED_FLASH_MODE, WIRED_FLASH_FREQUENCY, WIRED_FLASH_SIZE
    ));

    // Auto boot health: owner should not need a separate "Boot 修复" button.
    // Probe 0x0; full factory flash always rewrites bootloader when missing/corrupt.
    match probe_wired_bootloader_magic(&mut flasher) {
        Ok(WiredBootProbe::Present { magic }) => {
            log.push_str(&format!(
                "Boot check: present (magic=0x{magic:02x} at 0x0); full flash will still refresh bootloader/partition/app.\n"
            ));
        }
        Ok(WiredBootProbe::MissingOrCorrupt { detail }) => {
            log.push_str(&format!(
                "Boot check: missing/corrupt ({detail}); full flash will auto-repair bootloader at 0x0 then write partition table + app.\n"
            ));
            emit_wired_firmware_stage(
                progress_app_ref,
                "flash",
                "preparing",
                Some(&loaded.version),
                Some(&port),
                20,
                "Boot missing/corrupt — auto-repairing via full factory flash",
            );
        }
        Err(err) => {
            log.push_str(&format!(
                "Boot check: probe skipped ({err}); full flash will still write bootloader.bin at 0x0.\n"
            ));
        }
    }

    if !preserve_ota_data {
        if let Some((offset, size)) = loaded.otadata_region.as_ref() {
            let offset = parse_flash_u32_arg(offset, "otadata offset", true)?;
            let size = parse_flash_u32_arg(size, "otadata size", false)?;
            emit_wired_firmware_stage(
                progress_app_ref,
                "flash",
                "erasing",
                Some(&loaded.version),
                Some(&port),
                22,
                "Erasing OTA state partition",
            );
            flasher
                .erase_region(offset, size)
                .map_err(|err| format!("Failed to erase otadata region: {err}"))?;
            log.push_str(&format!(
                "Erased otadata region at 0x{offset:x}, size 0x{size:x}.\n"
            ));
            emit_wired_firmware_stage(
                progress_app_ref,
                "flash",
                "erased",
                Some(&loaded.version),
                Some(&port),
                26,
                "OTA state partition erased",
            );
        }
    } else {
        log.push_str("Preserved otadata region.\n");
        emit_wired_firmware_stage(
            progress_app_ref,
            "flash",
            "erased",
            Some(&loaded.version),
            Some(&port),
            26,
            "OTA state partition preserved",
        );
    }

    emit_wired_firmware_stage(
        progress_app_ref,
        "flash",
        "preparing",
        Some(&loaded.version),
        Some(&port),
        28,
        "Preparing firmware images",
    );
    let prepared = prepare_wired_flash_artifacts(&loaded, chip)?;
    let segments = prepared
        .iter()
        .map(|artifact| RomSegment {
            addr: artifact.offset,
            data: Cow::Borrowed(artifact.bytes.as_slice()),
        })
        .collect::<Vec<_>>();
    if progress_app_ref.is_some() {
        let progress_artifacts = wired_progress_artifacts_from_prepared(&prepared);
        let mut progress = WiredFlashProgressCallbacks::new(
            progress_app_ref,
            "flash",
            &loaded.version,
            &port,
            progress_artifacts,
            28,
            98,
        );
        flasher
            .write_bins_to_flash(&segments, Some(&mut progress))
            .map_err(|err| format!("Failed to write wired firmware image: {err}"))?;
    } else {
        flasher
            .write_bins_to_flash(&segments, None)
            .map_err(|err| format!("Failed to write wired firmware image: {err}"))?;
    }
    for artifact in &prepared {
        log.push_str(&format!(
            "Wrote {} ({}) at 0x{:x}, {} bytes",
            artifact.role,
            artifact.file,
            artifact.offset,
            artifact.bytes.len()
        ));
        if let Some(report) = artifact.patch_report {
            if report.changed {
                log.push_str("; ESP image header patched to wired flash config");
                if report.digest_recalculated {
                    log.push_str("; SHA256 digest recalculated");
                }
            }
        }
        log.push_str(".\n");
    }
    emit_wired_firmware_stage(
        progress_app_ref,
        "flash",
        "finalizing",
        Some(&loaded.version),
        Some(&port),
        99,
        "Finalizing wired firmware flash",
    );
    log.push_str("Wired factory flash completed and verified by espflash.\n");

    emit_wired_firmware_stage(
        progress_app_ref,
        "flash",
        "done",
        Some(&loaded.version),
        Some(&port),
        100,
        "Wired factory flash completed",
    );
    Ok(WiredFirmwareFlashResult {
        action: "flash".to_string(),
        kind: loaded.kind.as_str().to_string(),
        port,
        version: loaded.version,
        log: trim_command_output(&log, 24_000),
    })
}

pub fn run_wired_bootloader_repair(
    path: &Path,
    requested_port: Option<&str>,
    baud: Option<u32>,
) -> Result<WiredFirmwareFlashResult, String> {
    run_wired_bootloader_repair_with_progress(path, requested_port, baud, None)
}

pub fn run_wired_bootloader_repair_with_progress(
    path: &Path,
    requested_port: Option<&str>,
    baud: Option<u32>,
    progress_app: Option<AppHandle>,
) -> Result<WiredFirmwareFlashResult, String> {
    let progress_app_ref = progress_app.as_ref();
    emit_wired_firmware_stage(
        progress_app_ref,
        "bootloaderRepair",
        "loading",
        None,
        requested_port,
        1,
        "Loading wired firmware package",
    );
    let loaded = load_wired_firmware_package_internal(path)?;
    let chip = wired_target_chip(&loaded.target)?;
    let port_hint = normalize_requested_wired_port(requested_port);
    let bootloader = require_artifact(&loaded.artifacts, "bootloader")?;
    let bootloader_offset = parse_flash_u32_arg(&bootloader.offset, "bootloader offset", true)?;
    let bootloader_bytes = loaded
        .files
        .get(&bootloader.file)
        .ok_or_else(|| format!("Factory artifact file is missing: {}", bootloader.file))?;
    let (bootloader_bytes, patch_report) =
        prepare_esp_image_for_wired_flash("bootloader", bootloader_bytes, chip)?;
    emit_wired_firmware_stage(
        progress_app_ref,
        "bootloaderRepair",
        "packageLoaded",
        Some(&loaded.version),
        port_hint.as_deref(),
        4,
        "Bootloader image loaded",
    );
    let bauds = baud
        .map(|value| vec![value])
        .unwrap_or_else(|| WIRED_BOOT_REPAIR_BAUDS.to_vec());
    let reset_modes = [ResetBeforeOperation::DefaultReset];
    let mut log = String::new();
    log.push_str("> Listener Type built-in bootloader repair\n");
    log.push_str("No ESP-IDF, IDF_PATH, Python, or esptool.py environment is required.\n");
    log.push_str(
        "Repair strategy: poll for the COM port, use default reset to enter the ESP ROM loader, and write bootloader.bin in the same session.\n",
    );
    let mut failures = Vec::new();

    for baud in bauds {
        for before in reset_modes {
            emit_wired_firmware_stage(
                progress_app_ref,
                "bootloaderRepair",
                "connecting",
                Some(&loaded.version),
                port_hint.as_deref(),
                8,
                "Waiting for ESP32-S3 bootloader repair window",
            );
            match connect_builtin_esp_flasher_with_wait(
                port_hint.as_deref(),
                baud,
                chip,
                false,
                true,
                before,
                ResetAfterOperation::HardReset,
                WIRED_BOOT_REPAIR_CONNECT_TIMEOUT,
                true,
            ) {
                Ok((mut flasher, port, connect_log)) => {
                    log.push_str(&connect_log);
                    flasher.set_flash_size(WIRED_FLASH_SIZE);
                    emit_wired_firmware_stage(
                        progress_app_ref,
                        "bootloaderRepair",
                        "connected",
                        Some(&loaded.version),
                        Some(&port),
                        28,
                        "Connected to ESP32-S3 bootloader repair window",
                    );
                    log.push_str(&format!(
                        "Writing bootloader immediately at 0x{bootloader_offset:x}, {} bytes.\n",
                        bootloader_bytes.len()
                    ));
                    if patch_report.changed {
                        log.push_str("ESP image header patched to wired flash config");
                        if patch_report.digest_recalculated {
                            log.push_str("; SHA256 digest recalculated");
                        }
                        log.push_str(".\n");
                    }
                    let write_result = if progress_app_ref.is_some() {
                        let mut progress = WiredFlashProgressCallbacks::new(
                            progress_app_ref,
                            "bootloaderRepair",
                            &loaded.version,
                            &port,
                            vec![WiredFlashProgressArtifact {
                                role: bootloader.role.clone(),
                                file: bootloader.file.clone(),
                                offset: bootloader_offset,
                                size_bytes: bootloader_bytes.len() as u64,
                            }],
                            32,
                            96,
                        );
                        flasher.write_bin_to_flash(
                            bootloader_offset,
                            &bootloader_bytes,
                            Some(&mut progress),
                        )
                    } else {
                        flasher.write_bin_to_flash(bootloader_offset, &bootloader_bytes, None)
                    };
                    match write_result {
                        Ok(()) => {
                            emit_wired_firmware_stage(
                                progress_app_ref,
                                "bootloaderRepair",
                                "done",
                                Some(&loaded.version),
                                Some(&port),
                                100,
                                "Bootloader repair completed",
                            );
                            log.push_str("Bootloader repair completed and verified by espflash.\n");
                            return Ok(WiredFirmwareFlashResult {
                                action: "bootloaderRepair".to_string(),
                                kind: loaded.kind.as_str().to_string(),
                                port,
                                version: loaded.version,
                                log: trim_command_output(&log, 24_000),
                            });
                        }
                        Err(err) => failures.push(format!(
                            "baud {baud} before={before:?} bootloader write failed: {err}"
                        )),
                    }
                }
                Err(err) => failures.push(format!(
                    "baud {baud} before={before:?} connect failed: {err}"
                )),
            }
        }
    }

    Err(format!(
        "Bootloader repair failed after polling the COM port and trying all repair modes: {}",
        failures.join("; ")
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum WiredBootProbe {
    Present { magic: u8 },
    MissingOrCorrupt { detail: String },
}

const WIRED_BOOT_IMAGE_MAGIC: u8 = 0xe9;

fn classify_wired_bootloader_bytes(bytes: &[u8]) -> WiredBootProbe {
    if bytes.is_empty() {
        return WiredBootProbe::MissingOrCorrupt {
            detail: "empty read at 0x0".to_string(),
        };
    }
    let magic = bytes[0];
    if magic == WIRED_BOOT_IMAGE_MAGIC {
        WiredBootProbe::Present { magic }
    } else {
        WiredBootProbe::MissingOrCorrupt {
            detail: format!("magic=0x{magic:02x}, expected 0x{WIRED_BOOT_IMAGE_MAGIC:02x}"),
        }
    }
}

/// Read a few bytes at 0x0 and decide whether a usable ESP bootloader image is present.
fn probe_wired_bootloader_magic(flasher: &mut Flasher) -> Result<WiredBootProbe, String> {
    const BOOT_PROBE_BYTES: u32 = 16;
    const BOOT_PROBE_BLOCK: u32 = 16;

    let temp_path = std::env::temp_dir().join(format!(
        "listener-boot-probe-{}-{}.bin",
        std::process::id(),
        Instant::now().elapsed().as_nanos()
    ));
    let read_result =
        flasher.read_flash(0, BOOT_PROBE_BYTES, BOOT_PROBE_BLOCK, 1, temp_path.clone());
    let bytes = match read_result {
        Ok(()) => std::fs::read(&temp_path).map_err(|err| {
            let _ = std::fs::remove_file(&temp_path);
            format!("boot probe read file failed: {err}")
        }),
        Err(err) => {
            let _ = std::fs::remove_file(&temp_path);
            return Err(format!("boot probe read_flash failed: {err}"));
        }
    };
    let _ = std::fs::remove_file(&temp_path);
    Ok(classify_wired_bootloader_bytes(&bytes?))
}

pub fn package_manifest_baud(package: &LoadedWiredFirmwarePackage) -> Option<u32> {
    let manifest = serde_json::from_str::<FactoryFirmwareManifest>(&package.manifest_text).ok()?;
    manifest
        .flash
        .as_ref()
        .and_then(|flash| flash.baud.as_ref())
        .and_then(parse_baud_value)
}

#[cfg(test)]
mod boot_probe_tests {
    use super::{
        classify_wired_bootloader_bytes, WiredBootProbe, FIRMWARE_OTA_POST_READY_FAST_PATH_TIMEOUT,
        FIRMWARE_OTA_POST_READY_TIMEOUT, WIRED_BOOT_IMAGE_MAGIC,
    };

    #[test]
    fn classifies_esp_image_magic_as_present() {
        assert_eq!(
            classify_wired_bootloader_bytes(&[WIRED_BOOT_IMAGE_MAGIC, 0, 1, 2]),
            WiredBootProbe::Present {
                magic: WIRED_BOOT_IMAGE_MAGIC
            }
        );
    }

    #[test]
    fn classifies_empty_or_wrong_magic_as_missing() {
        assert!(matches!(
            classify_wired_bootloader_bytes(&[]),
            WiredBootProbe::MissingOrCorrupt { .. }
        ));
        assert!(matches!(
            classify_wired_bootloader_bytes(&[0xff, 0, 0]),
            WiredBootProbe::MissingOrCorrupt { .. }
        ));
    }

    #[test]
    fn post_ota_notify_reuse_has_a_short_failure_budget_before_long_recovery() {
        assert_eq!(
            FIRMWARE_OTA_POST_READY_FAST_PATH_TIMEOUT,
            std::time::Duration::from_millis(3000)
        );
        assert!(FIRMWARE_OTA_POST_READY_FAST_PATH_TIMEOUT < FIRMWARE_OTA_POST_READY_TIMEOUT);
    }
}

pub fn parse_baud_value(value: &Value) -> Option<u32> {
    if let Some(number) = value.as_u64() {
        return u32::try_from(number).ok();
    }
    value
        .as_str()
        .and_then(|text| text.trim().parse::<u32>().ok())
}

pub fn wired_target_chip(target: &str) -> Result<Chip, String> {
    match wired_firmware_target_kind(target)? {
        WiredFirmwareTargetKind::Esp32S3 => Ok(Chip::Esp32s3),
    }
}

pub fn wired_firmware_target_kind(target: &str) -> Result<WiredFirmwareTargetKind, String> {
    let normalized = target
        .trim()
        .to_ascii_lowercase()
        .replace('-', "")
        .replace('_', "");
    match normalized.as_str() {
        "esp32s3" => Ok(WiredFirmwareTargetKind::Esp32S3),
        _ => Err(format!(
            "Wired firmware flashing supports ESP32-S3 Listener packages; package target is {target}."
        )),
    }
}

pub fn prepare_wired_flash_artifacts(
    package: &LoadedWiredFirmwarePackage,
    chip: Chip,
) -> Result<Vec<PreparedWiredFlashArtifact>, String> {
    let mut prepared = Vec::new();
    for role in ["bootloader", "partition_table", "app"] {
        let artifact = require_artifact(&package.artifacts, role)?;
        let offset = parse_flash_u32_arg(&artifact.offset, &format!("{role} offset"), true)?;
        let bytes = package
            .files
            .get(&artifact.file)
            .ok_or_else(|| format!("Factory artifact file is missing: {}", artifact.file))?;
        let (bytes, patch_report) = if matches!(role, "bootloader" | "app") {
            let (bytes, report) = prepare_esp_image_for_wired_flash(role, bytes, chip)?;
            (bytes, Some(report))
        } else {
            (bytes.clone(), None)
        };
        prepared.push(PreparedWiredFlashArtifact {
            role: artifact.role.clone(),
            file: artifact.file.clone(),
            offset,
            bytes,
            patch_report,
        });
    }
    Ok(prepared)
}

pub fn wired_progress_artifacts_from_prepared(
    prepared: &[PreparedWiredFlashArtifact],
) -> Vec<WiredFlashProgressArtifact> {
    prepared
        .iter()
        .map(|artifact| WiredFlashProgressArtifact {
            role: artifact.role.clone(),
            file: artifact.file.clone(),
            offset: artifact.offset,
            size_bytes: artifact.bytes.len() as u64,
        })
        .collect()
}

pub fn prepare_esp_image_for_wired_flash(
    role: &str,
    bytes: &[u8],
    chip: Chip,
) -> Result<(Vec<u8>, EspImagePatchReport), String> {
    const ESP_IMAGE_MAGIC: u8 = 0xe9;
    const ESP_IMAGE_APPEND_DIGEST_OFFSET: usize = 23;
    const ESP_IMAGE_DIGEST_LEN: usize = 32;

    if bytes.len() <= ESP_IMAGE_APPEND_DIGEST_OFFSET {
        return Err(format!(
            "Factory artifact {role} is too small to be an ESP image."
        ));
    }
    if bytes[0] != ESP_IMAGE_MAGIC {
        return Err(format!(
            "Factory artifact {role} is not an ESP image; expected magic 0xe9."
        ));
    }

    let mode = WIRED_FLASH_MODE as u8;
    let size = WIRED_FLASH_SIZE
        .encode_flash_size()
        .map_err(|err| format!("Unsupported wired flash size: {err}"))?;
    let frequency = WIRED_FLASH_FREQUENCY
        .encode_flash_frequency(chip)
        .map_err(|err| format!("Unsupported wired flash frequency: {err}"))?;
    let flash_config = (size << 4) | frequency;

    let mut patched = bytes.to_vec();
    let changed = patched[2] != mode || patched[3] != flash_config;
    let mut digest_recalculated = false;
    if changed {
        patched[2] = mode;
        patched[3] = flash_config;
        if patched[ESP_IMAGE_APPEND_DIGEST_OFFSET] == 1 {
            if patched.len() <= ESP_IMAGE_DIGEST_LEN {
                return Err(format!(
                    "Factory artifact {role} declares a SHA256 digest but is too small to contain one."
                ));
            }
            let digest_start = patched.len() - ESP_IMAGE_DIGEST_LEN;
            let digest = sha256_digest_bytes(&patched[..digest_start]);
            patched[digest_start..].copy_from_slice(&digest);
            digest_recalculated = true;
        }
    }

    Ok((
        patched,
        EspImagePatchReport {
            changed,
            digest_recalculated,
        },
    ))
}

pub fn sha256_digest_bytes(bytes: &[u8]) -> [u8; 32] {
    let digest = Sha256::digest(bytes);
    let mut output = [0_u8; 32];
    output.copy_from_slice(&digest);
    output
}

pub fn normalize_esptool_region_arg(
    value: &str,
    field_name: &str,
    allow_zero: bool,
) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("Factory manifest {field_name} is empty."));
    }
    if let Some(hex) = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
    {
        let parsed = u64::from_str_radix(hex, 16).map_err(|_| {
            format!("Factory manifest {field_name} must be a number, hex value, or IDF size unit.")
        })?;
        if parsed == 0 && !allow_zero {
            return Err(format!(
                "Factory manifest {field_name} must be greater than 0."
            ));
        }
        return Ok(format!("0x{parsed:x}"));
    }
    if let Ok(parsed) = trimmed.parse::<u64>() {
        if parsed == 0 && !allow_zero {
            return Err(format!(
                "Factory manifest {field_name} must be greater than 0."
            ));
        }
        return Ok(parsed.to_string());
    }

    let upper = trimmed.to_ascii_uppercase();
    let (number, multiplier) = if let Some(number) = upper.strip_suffix("KB") {
        (number, 1024_u64)
    } else if let Some(number) = upper.strip_suffix('K') {
        (number, 1024_u64)
    } else if let Some(number) = upper.strip_suffix("MB") {
        (number, 1024_u64 * 1024)
    } else if let Some(number) = upper.strip_suffix('M') {
        (number, 1024_u64 * 1024)
    } else {
        return Err(format!(
            "Factory manifest {field_name} must be a number, hex value, or IDF size unit."
        ));
    };
    let count = number.trim().parse::<u64>().map_err(|_| {
        format!("Factory manifest {field_name} has invalid IDF size unit: {trimmed}.")
    })?;
    let parsed = count
        .checked_mul(multiplier)
        .ok_or_else(|| format!("Factory manifest {field_name} is too large: {trimmed}."))?;
    if parsed == 0 && !allow_zero {
        return Err(format!(
            "Factory manifest {field_name} must be greater than 0."
        ));
    }
    Ok(parsed.to_string())
}

pub fn parse_flash_u32_arg(value: &str, field_name: &str, allow_zero: bool) -> Result<u32, String> {
    let normalized = normalize_esptool_region_arg(value, field_name, allow_zero)?;
    let parsed = if let Some(hex) = normalized
        .strip_prefix("0x")
        .or_else(|| normalized.strip_prefix("0X"))
    {
        u64::from_str_radix(hex, 16).map_err(|_| {
            format!("Factory manifest {field_name} must be a number, hex value, or IDF size unit.")
        })?
    } else {
        normalized.parse::<u64>().map_err(|_| {
            format!("Factory manifest {field_name} must be a number, hex value, or IDF size unit.")
        })?
    };
    u32::try_from(parsed)
        .map_err(|_| format!("Factory manifest {field_name} is too large: {normalized}."))
}

pub fn normalize_requested_wired_port(requested: Option<&str>) -> Option<String> {
    requested
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .filter(|value| !value.eq_ignore_ascii_case("COMx"))
        .map(ToOwned::to_owned)
}

pub fn connect_builtin_esp_flasher_with_wait(
    requested_port: Option<&str>,
    baud: u32,
    chip: Chip,
    use_stub: bool,
    verify: bool,
    before_operation: ResetBeforeOperation,
    after_operation: ResetAfterOperation,
    timeout: Duration,
    allow_auto_select: bool,
) -> Result<(Flasher, String, String), String> {
    let started = Instant::now();
    let mut attempts = 0_u32;
    let mut connection_errors = Vec::new();

    loop {
        match select_wired_flash_port_for_attempt(requested_port, allow_auto_select) {
            Ok(Some(port)) => {
                attempts = attempts.saturating_add(1);
                match connect_builtin_esp_flasher(
                    &port,
                    baud,
                    chip,
                    use_stub,
                    verify,
                    before_operation,
                    after_operation,
                ) {
                    Ok(flasher) => {
                        let mut log = String::new();
                        log.push_str(&format!(
                            "Serial port: {port}\nConnecting with built-in espflash: baud={baud}, stub={use_stub}, verify={verify}, before={before_operation:?}, after={after_operation:?}.\n"
                        ));
                        log.push_str(&format!("Connected after {attempts} attempt(s).\n"));
                        return Ok((flasher, port, log));
                    }
                    Err(err) => {
                        connection_errors.push(format!("{port}: {err}"));
                    }
                }
            }
            Ok(None) => {
                connection_errors.push("serial port not present yet".to_string());
            }
            Err(err) => return Err(err),
        }

        if started.elapsed() >= timeout {
            break;
        }
        std::thread::sleep(WIRED_FLASH_CONNECT_RETRY_INTERVAL);
    }

    let requested = normalize_requested_wired_port(requested_port)
        .map(|port| format!(" Requested port: {port}."))
        .unwrap_or_default();
    let last_error = connection_errors
        .last()
        .map(|err| format!(" Last error: {err}."))
        .unwrap_or_default();
    Err(format!(
        "Timed out after {:.1}s waiting for a Listener ESP32-S3 serial flashing connection.{requested}{last_error}",
        timeout.as_secs_f32()
    ))
}

pub fn connect_builtin_esp_flasher(
    port: &str,
    baud: u32,
    chip: Chip,
    use_stub: bool,
    verify: bool,
    before_operation: ResetBeforeOperation,
    after_operation: ResetAfterOperation,
) -> Result<Flasher, String> {
    let usb_info = usb_port_info_for(port);
    let serial_port = serialport::new(port, 115_200)
        .flow_control(FlowControl::None)
        .timeout(WIRED_FLASH_SERIAL_IO_TIMEOUT)
        .open_native()
        .map_err(|err| format!("Failed to open serial port {port}: {err}"))?;

    Flasher::connect(
        serial_port,
        usb_info,
        Some(baud),
        use_stub,
        verify,
        false,
        Some(chip),
        after_operation,
        before_operation,
    )
    .map_err(|err| format!("Failed to connect to ESP ROM/flasher on {port}: {err}"))
}

pub fn select_wired_flash_port_for_attempt(
    requested_port: Option<&str>,
    allow_auto_select: bool,
) -> Result<Option<String>, String> {
    if let Some(port) = normalize_requested_wired_port(requested_port) {
        if allow_auto_select {
            let ports = list_wired_firmware_ports_internal();
            if ports
                .iter()
                .any(|candidate| candidate.port.eq_ignore_ascii_case(&port))
            {
                return Ok(Some(port));
            }
            if ports.is_empty() {
                return Ok(None);
            }
            let esp32_ports = ports
                .iter()
                .filter(|candidate| candidate.is_likely_esp32)
                .collect::<Vec<_>>();
            if esp32_ports.len() == 1 {
                return Ok(Some(esp32_ports[0].port.clone()));
            }
            if ports.len() == 1 {
                return Ok(Some(ports[0].port.clone()));
            }

            let summary = ports
                .iter()
                .map(|candidate| candidate.label.clone())
                .collect::<Vec<_>>()
                .join("; ");
            return Err(format!(
                "Requested serial port {port} is not present, and multiple serial ports were detected after USB re-enumeration. Choose the Listener COM port explicitly. Ports: {summary}"
            ));
        }
        return Ok(Some(port));
    }
    if !allow_auto_select {
        return resolve_wired_flash_port(None).map(Some);
    }

    let ports = list_wired_firmware_ports_internal();
    if ports.is_empty() {
        return Ok(None);
    }
    let esp32_ports = ports
        .iter()
        .filter(|port| port.is_likely_esp32)
        .collect::<Vec<_>>();
    if esp32_ports.len() == 1 {
        return Ok(Some(esp32_ports[0].port.clone()));
    }
    if ports.len() == 1 {
        return Ok(Some(ports[0].port.clone()));
    }

    let summary = ports
        .iter()
        .map(|port| port.label.clone())
        .collect::<Vec<_>>()
        .join("; ");
    Err(format!(
        "Multiple serial ports were detected. Choose the Listener COM port explicitly. Ports: {summary}"
    ))
}

pub fn usb_port_info_for(port_name: &str) -> UsbPortInfo {
    if let Ok(ports) = serialport::available_ports() {
        for port in ports {
            if port.port_name.eq_ignore_ascii_case(port_name) {
                if let SerialPortType::UsbPort(info) = port.port_type {
                    return info;
                }
            }
        }
    }
    UsbPortInfo {
        vid: 0,
        pid: 0,
        serial_number: None,
        manufacturer: None,
        product: None,
    }
}

pub fn format_wired_device_info(info: &espflash::flasher::DeviceInfo) -> String {
    let revision = info
        .revision
        .map(|(major, minor)| format!("{major}.{minor}"))
        .unwrap_or_else(|| "unknown".to_string());
    let features = if info.features.is_empty() {
        "none".to_string()
    } else {
        info.features.join(", ")
    };
    format!(
        "Connected chip: {}\nRevision: {revision}\nCrystal: {:?}\nMAC: {}\nFeatures: {features}\n",
        info.chip, info.crystal_frequency, info.mac_address
    )
}

pub fn trim_command_output(text: &str, max_chars: usize) -> String {
    let chars = text.chars().collect::<Vec<_>>();
    if chars.len() <= max_chars {
        return text.to_string();
    }
    let tail = chars[chars.len().saturating_sub(max_chars)..]
        .iter()
        .collect::<String>();
    format!("... output truncated ...\n{tail}")
}

pub async fn transfer_firmware_ota_ble(
    app: AppHandle,
    coord: CoordinatorState<'_>,
    manifest: Value,
    firmware_bytes: Vec<u8>,
    expected_sha256: String,
) -> Result<FirmwareOtaBleTransferResult, String> {
    let total_started = Instant::now();
    let phase = coord.dictation_phase_for_cli();
    if phase != SessionPhase::Idle {
        return Err(format!(
            "Recording or dictation is still active ({phase:?}). Stop it before updating firmware."
        ));
    }
    let manifest = serde_json::from_value::<crate::firmware_ota::FirmwareOtaManifest>(manifest)
        .map_err(|err| format!("OTA manifest payload is invalid: {err}"))
        .and_then(crate::firmware_ota::validate_normalized_manifest)?;
    if firmware_bytes.is_empty() {
        return Err("firmware_ota.bin is empty.".to_string());
    }
    if firmware_bytes.len() as u64 != manifest.file_size_bytes {
        return Err(format!(
            "firmware_ota.bin size changed before transfer: manifest={} actual={}",
            manifest.file_size_bytes,
            firmware_bytes.len()
        ));
    }
    if !expected_sha256.eq_ignore_ascii_case(&manifest.file_sha256) {
        return Err("OTA package hash changed before transfer.".to_string());
    }
    let actual_sha256 = crate::firmware_ota::sha256_hex(&firmware_bytes);
    if actual_sha256 != manifest.file_sha256 {
        return Err("firmware_ota.bin SHA256 does not match ota_manifest.json.".to_string());
    }

    let pretransfer_type_ready_started = Instant::now();
    let pretransfer_type_ready = coord
        .wait_for_embedded_ble_listener_ready_before_firmware_ota(
            FIRMWARE_OTA_PRETRANSFER_READY_TIMEOUT,
        )
        .await?;
    let pretransfer_type_ready_elapsed_ms = elapsed_ms_u64(pretransfer_type_ready_started);
    log::info!(
        "[firmware-ota] pre-transfer Listener notify ready={} elapsed_ms={}",
        pretransfer_type_ready,
        pretransfer_type_ready_elapsed_ms
    );
    // Send TYPE:OTA while audio notify is still live so firmware latches the
    // Type OTA lease (LED stays Type-ready; TYPE:BYE during pause is ignored).
    // Doing this after try_begin/suppress used to BYE first → find-Type LED and
    // flaky handoff timeouts.
    let mut observability = crate::observability::begin_ota_transfer();
    if let Err(first_error) = crate::embedded_ble::request_listener_ota_v1_active_link(Some(
        observability.correlation_id(),
    )) {
        log::warn!(
            "[firmware-ota] Listener OTA handoff lost the ready capture; waiting once for notify recovery: {first_error}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
        let retry_result = coord
            .wait_for_embedded_ble_listener_ready_before_firmware_ota(
                FIRMWARE_OTA_HANDOFF_RETRY_TIMEOUT,
            )
            .await
            .and_then(|_| crate::embedded_ble::request_listener_ota_v1_active_link(None));
        if let Err(retry_error) = retry_result {
            let error = format!(
                "Listener OTA control handoff failed after one notify-ready retry: first={first_error}; retry={retry_error}"
            );
            observability.record_control_handoff_failed(&error);
            return Err(error);
        }
        log::info!("[firmware-ota] Listener OTA reconnect handoff recovered on bounded retry");
    }
    log::info!("[firmware-ota] Listener OTA v1 reconnect handoff accepted while notify still live");
    if !coord.try_begin_firmware_ota_transfer() {
        return Err("Firmware OTA is already in progress.".to_string());
    }
    let listener_release_started = Instant::now();
    if !coord
        .pause_embedded_ble_listener_for_ota(FIRMWARE_OTA_LISTENER_RELEASE_TIMEOUT)
        .await
    {
        let error = format!(
            "Listener BLE audio notification did not release within {} ms before OTA target preparation.",
            FIRMWARE_OTA_LISTENER_RELEASE_TIMEOUT.as_millis()
        );
        log::error!("[firmware-ota] {error}");
        observability.record_transfer_failed(elapsed_ms_u64(listener_release_started), &error);
        coord.end_failed_firmware_ota_transfer();
        return Err(error);
    }
    log::info!(
        "[firmware-ota] Listener notify fully released before OTA target preparation elapsed_ms={}",
        elapsed_ms_u64(listener_release_started)
    );
    let version = manifest.version;
    let manifest_chunk_bytes = manifest.gatt_chunk_bytes as usize;
    let transfer_sha256 = expected_sha256.clone();
    let (target_ready_tx, target_ready_rx) = mpsc::sync_channel(1);
    let (start_transfer_tx, start_transfer_rx) = mpsc::sync_channel(1);
    let app_for_progress = app;
    let target_prepare_started = Instant::now();
    let transfer_task = tauri::async_runtime::spawn_blocking(move || {
        let progress = |bytes_sent, bytes_total| {
            let _ = app_for_progress.emit(
                "firmware-ota:progress",
                serde_json::json!({
                    "bytesSent": bytes_sent,
                    "bytesTotal": bytes_total,
                }),
            );
        };
        crate::embedded_ble::transfer_listener_ota_v1_after_active_link_hint_staged(
            target_ready_tx,
            start_transfer_rx,
            FIRMWARE_OTA_TARGET_PREPARE_TIMEOUT,
            &transfer_sha256,
            &firmware_bytes,
            manifest_chunk_bytes,
            Some(&progress),
        )
    });
    let target_prepare = tauri::async_runtime::spawn_blocking(move || {
        target_ready_rx.recv_timeout(FIRMWARE_OTA_TARGET_PREPARE_TIMEOUT)
    })
    .await
    .map_err(|err| format!("Listener BLE OTA target preparation wait task failed: {err}"))
    .and_then(|result| {
        result.map_err(|err| format!("Listener BLE OTA target preparation timed out: {err}"))
    })
    .and_then(|result| result);
    let target_prepare_elapsed_ms = elapsed_ms_u64(target_prepare_started);
    if let Err(error) = target_prepare {
        drop(start_transfer_tx);
        let _ = transfer_task.await;
        log::error!(
            "[firmware-ota] transfer prepare failed elapsed_ms={target_prepare_elapsed_ms}: {error}"
        );
        observability.record_transfer_failed(target_prepare_elapsed_ms, &error);
        coord.end_failed_firmware_ota_transfer();
        return Err(error);
    }
    log::info!(
        "[firmware-ota] Listener OTA v1 target prepared after listener release elapsed_ms={target_prepare_elapsed_ms}"
    );
    if start_transfer_tx.send(()).is_err() {
        let error = "Listener BLE OTA target closed before transfer start.".to_string();
        let _ = transfer_task.await;
        log::error!(
            "[firmware-ota] transfer start failed elapsed_ms={target_prepare_elapsed_ms}: {error}"
        );
        observability.record_transfer_failed(target_prepare_elapsed_ms, &error);
        coord.end_failed_firmware_ota_transfer();
        return Err(error);
    }
    let transfer_started = Instant::now();
    let transfer = transfer_task
        .await
        .map_err(|err| format!("Listener BLE OTA transfer task failed: {err}"))
        .and_then(|result| result);
    let transfer_wall_elapsed_ms = elapsed_ms_u64(transfer_started);
    match &transfer {
        Ok(stats) => {
            log::info!(
                "[firmware-ota] transfer ok protocol_ms={} wall_ms={transfer_wall_elapsed_ms} fixed_ms={} bytes={} transport={}",
                stats.protocol_transfer_elapsed_ms,
                transfer_wall_elapsed_ms.saturating_sub(stats.protocol_transfer_elapsed_ms),
                stats.bytes_transferred,
                stats.transport
            );
            observability.record_transfer_completed(stats.protocol_transfer_elapsed_ms);
        }
        Err(error) => {
            // Always keep the full host error string in listener-type.log. UI maps
            // unknown failures to "设备拒绝升级"; without this line operators only see
            // obs-v1 category=transport and cannot tell timeout vs ATT protocol_error.
            log::error!(
                "[firmware-ota] transfer failed elapsed_ms={transfer_wall_elapsed_ms}: {error}"
            );
            observability.record_transfer_failed(transfer_wall_elapsed_ms, error);
        }
    }
    let confirm = if transfer.is_ok() {
        confirm_listener_ota_v1_reachable(&version).await
    } else {
        FirmwareOtaConfirmOutcome {
            confirmed_version: None,
            elapsed_ms: 0,
            attempts: 0,
            matched: false,
        }
    };
    if transfer.is_ok() {
        // Success path: clear OTA exclusive gate only. Do not start a first listener
        // here — it races reboot/CCCD settle, burns gen N, then after-ota refresh
        // cancels it and the owner sees "must restart Type".
        coord.end_firmware_ota_transfer_with_listener_restore(false);
        observability.record_reconnect_confirmation(confirm.matched);
    } else {
        // Failed BEGIN/data/finish: restore only through a generation-scoped bonded
        // recovery. OTA disconnect/CCCD errors are not evidence of stale pairing.
        coord.end_failed_firmware_ota_transfer();
    }

    let stats = transfer?;
    let transfer_elapsed_ms = stats.protocol_transfer_elapsed_ms;
    let transfer_fixed_elapsed_ms = transfer_wall_elapsed_ms.saturating_sub(transfer_elapsed_ms);
    if !confirm.matched {
        coord.refresh_embedded_ble_listener();
        let error = format!(
            "Listener firmware version {} was not confirmed after OTA.",
            version
        );
        log::error!("[firmware-ota] post-transfer version confirm failed: {error}");
        return Err(error);
    }
    crate::embedded_ble::request_listener_ota_post_confirm_notify_fast_retry();
    coord.refresh_embedded_ble_listener_after_firmware_ota();
    let type_ready_started = Instant::now();
    let type_ready = match coord
        .wait_for_embedded_ble_listener_ready_after_firmware_ota(
            FIRMWARE_OTA_POST_READY_FAST_PATH_TIMEOUT,
        )
        .await
    {
        Ok(ready) => ready,
        Err(fast_path_error) => {
            // The verified CCCD reuse target can briefly become ready and then
            // disconnect as Windows releases the service-confirm probe. Restart
            // promptly; the old 28s first wait made a recoverable edge look stuck.
            log::warn!(
                "[firmware-ota] post-OTA fast notify path not stable within {} ms ({fast_path_error}); refreshing immediately",
                FIRMWARE_OTA_POST_READY_FAST_PATH_TIMEOUT.as_millis()
            );
            crate::embedded_ble::request_listener_ota_post_confirm_notify_fast_retry();
            coord.refresh_embedded_ble_listener_after_firmware_ota();
            match coord
                .wait_for_embedded_ble_listener_ready_after_firmware_ota(
                    FIRMWARE_OTA_POST_READY_TIMEOUT,
                )
                .await
            {
                Ok(ready) => ready,
                Err(first_error) => {
                    // Keep the long Windows re-enumeration fallback for devices
                    // that were not actually exposed when the fast path ran.
                    log::warn!(
                        "[firmware-ota] post-OTA Listener notify not ready within {} ms ({first_error}); refreshing listener and retrying",
                        FIRMWARE_OTA_POST_READY_TIMEOUT.as_millis()
                    );
                    crate::embedded_ble::request_listener_ota_post_confirm_notify_fast_retry();
                    coord.refresh_embedded_ble_listener_after_firmware_ota();
                    coord
                        .wait_for_embedded_ble_listener_ready_after_firmware_ota(
                            FIRMWARE_OTA_POST_READY_RETRY_TIMEOUT,
                        )
                        .await
                        .map_err(|retry_error| {
                            format!(
                                "Listener did not reattach after OTA. Fast path: {fast_path_error}. First wait: {first_error}. Retry: {retry_error}. If Windows still shows the device as paired, use 一键修复; if pairing was lost, pair Listener again in Type."
                            )
                        })?
                }
            }
        }
    };
    let type_ready_elapsed_ms = elapsed_ms_u64(type_ready_started);
    let non_transfer_fixed_elapsed_ms = target_prepare_elapsed_ms
        .saturating_add(transfer_fixed_elapsed_ms)
        .saturating_add(confirm.elapsed_ms)
        .saturating_add(type_ready_elapsed_ms);
    let total_elapsed_ms = elapsed_ms_u64(total_started);
    log::info!(
        "[firmware-ota] BLE OTA result transport={} bytes={} chunks={} pretransfer_type_ready={} pretransfer_type_ready_ms={} target_prepare_ms={} transfer_ms={} transfer_fixed_ms={} confirm_ms={} confirm_attempts={} confirm_matched={} type_ready={} type_ready_ms={} non_transfer_fixed_ms={} total_ms={} data_write_ms={} control_write_ms={} status_read_ms={}",
        stats.transport,
        stats.bytes_transferred,
        stats.chunks_sent,
        pretransfer_type_ready,
        pretransfer_type_ready_elapsed_ms,
        target_prepare_elapsed_ms,
        transfer_elapsed_ms,
        transfer_fixed_elapsed_ms,
        confirm.elapsed_ms,
        confirm.attempts,
        confirm.matched,
        type_ready,
        type_ready_elapsed_ms,
        non_transfer_fixed_elapsed_ms,
        total_elapsed_ms,
        stats.data_write_elapsed_ms,
        stats.control_write_elapsed_ms,
        stats.status_read_elapsed_ms
    );
    Ok(FirmwareOtaBleTransferResult {
        bytes_transferred: stats.bytes_transferred,
        chunks_sent: stats.chunks_sent,
        confirmed_version: confirm.confirmed_version,
        transport: stats.transport,
        pretransfer_type_ready,
        pretransfer_type_ready_elapsed_ms,
        target_prepare_elapsed_ms,
        transfer_elapsed_ms,
        confirm_elapsed_ms: confirm.elapsed_ms,
        type_ready,
        type_ready_elapsed_ms,
        non_transfer_fixed_elapsed_ms,
        total_elapsed_ms,
    })
}
