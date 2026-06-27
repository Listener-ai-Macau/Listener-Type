use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PROTOCOL_NAME: &str = "listener_ble_ota";
pub const FIRMWARE_CAPABILITY: &str = "firmware_ota_v1";
pub const OTA_FILE_NAME: &str = "firmware_ota.bin";
pub const OTA_SERVICE_UUID: &str = "710af845-6d9f-6583-0c4d-9e5b3bc3092a";
pub const OTA_CONTROL_UUID: &str = "710af845-6d9f-6583-0c4d-9e5b3bc3092b";
pub const OTA_DATA_UUID: &str = "710af845-6d9f-6583-0c4d-9e5b3bc3092c";
pub const OTA_MAX_CHUNK_BYTES: u64 = 500;
pub const OTA_CHUNK_BYTES: u64 = OTA_MAX_CHUNK_BYTES;
pub const STM32WB_ST_PROTOCOL_NAME: &str = "stm32wb_st_ble_ota";
pub const STM32WB_ST_FIRMWARE_CAPABILITY: &str = "stm32wb_st_ble_ota_v1";
pub const STM32WB_ST_OTA_SERVICE_UUID: &str = "8f7a0007-7b7d-4f3d-9d6f-6c2d1b7c0000";
pub const STM32WB_ST_OTA_CONTROL_UUID: &str = "8f7a7002-7b7d-4f3d-9d6f-6c2d1b7c0000";
pub const STM32WB_ST_OTA_DATA_UUID: &str = "8f7a7004-7b7d-4f3d-9d6f-6c2d1b7c0000";
pub const STM32WB_ST_OTA_CONFIRM_UUID: &str = "8f7a7003-7b7d-4f3d-9d6f-6c2d1b7c0000";
pub const STM32WB_ST_OTA_CHUNK_BYTES: u64 = 248;
pub const OTA_MAX_VERSION_CHARS: usize = 31;
pub const DEFAULT_CONFIRM_TIMEOUT: Duration = Duration::from_secs(45);
pub const CONFIRM_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FirmwareOtaManifest {
    pub schema_version: u64,
    pub package_type: String,
    pub project: String,
    pub version: String,
    pub protocol_name: String,
    pub protocol_version: u64,
    pub hardware_revision: String,
    pub min_desktop_version: String,
    pub channel: String,
    pub file_name: String,
    pub file_size_bytes: u64,
    pub file_sha256: String,
    pub firmware_capability: String,
    pub gatt_service_uuid: String,
    pub gatt_control_uuid: String,
    pub gatt_data_uuid: String,
    pub gatt_confirm_uuid: Option<String>,
    pub gatt_chunk_bytes: u64,
    pub rollback_instructions: Vec<String>,
    pub recovery_instructions: Vec<String>,
}

impl FirmwareOtaManifest {
    pub fn is_listener_ble_ota(&self) -> bool {
        self.protocol_name == PROTOCOL_NAME
    }

    pub fn is_stm32wb_st_ble_ota(&self) -> bool {
        self.protocol_name == STM32WB_ST_PROTOCOL_NAME
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FirmwareOtaValidationContext {
    pub desktop_version: String,
    pub expected_hardware_revision: String,
    pub current_firmware_version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FirmwareOtaPackageValidation {
    pub ok: bool,
    pub manifest: Option<FirmwareOtaManifest>,
    pub firmware_sha256: Option<String>,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct FirmwareOtaPackage {
    pub manifest_path: PathBuf,
    pub firmware_path: PathBuf,
    pub manifest: FirmwareOtaManifest,
    pub firmware_bytes: Vec<u8>,
    pub firmware_sha256: String,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FirmwareOtaHeadlessReport {
    pub status: String,
    pub mode: String,
    pub manifest_path: PathBuf,
    pub firmware_path: PathBuf,
    pub manifest: Option<FirmwareOtaManifest>,
    pub firmware_sha256: Option<String>,
    pub package_valid: bool,
    pub preflight: Option<FirmwareOtaHeadlessPreflight>,
    pub transfer: Option<FirmwareOtaHeadlessTransfer>,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FirmwareOtaHeadlessPreflight {
    pub recording_active: bool,
    pub dictation_phase: Option<String>,
    pub connected: bool,
    pub hardware_revision: Option<String>,
    pub firmware_version: Option<String>,
    pub capabilities: Vec<String>,
    pub battery_percent: Option<u8>,
    pub usb_powered: Option<bool>,
    pub detail: Option<String>,
    pub blockers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FirmwareOtaHeadlessTransfer {
    pub bytes_transferred: usize,
    pub transport: String,
    pub confirmed_version: Option<String>,
    pub version_confirmed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FirmwareOtaHeadlessOptions {
    pub manifest_path: PathBuf,
    pub firmware_path: PathBuf,
    pub preflight_only: bool,
    pub transfer: bool,
    pub desktop_version: String,
    pub expected_hardware_revision: String,
    pub current_firmware_version: Option<String>,
    pub recording_active: bool,
    pub dictation_phase: Option<String>,
}

pub async fn run_headless(options: FirmwareOtaHeadlessOptions) -> FirmwareOtaHeadlessReport {
    let mode = mode_name(&options).to_string();
    let context = FirmwareOtaValidationContext {
        desktop_version: options.desktop_version.clone(),
        expected_hardware_revision: options.expected_hardware_revision.clone(),
        current_firmware_version: options.current_firmware_version.clone(),
    };
    let package = match load_package(&options.manifest_path, &options.firmware_path, &context) {
        Ok(package) => package,
        Err(validation) => {
            return FirmwareOtaHeadlessReport {
                status: "FAIL".to_string(),
                mode,
                manifest_path: options.manifest_path,
                firmware_path: options.firmware_path,
                manifest: validation.manifest,
                firmware_sha256: validation.firmware_sha256,
                package_valid: false,
                preflight: None,
                transfer: None,
                errors: validation.errors,
                warnings: validation.warnings,
            };
        }
    };

    let mut errors = Vec::new();
    let warnings = package.warnings.clone();
    let mut preflight = None;
    let mut transfer = None;

    if options.transfer {
        let attempt = run_transfer_preflight_and_write(&package, &options);
        preflight = Some(attempt.preflight);
        errors.extend(attempt.errors);
        if let Some(stats) = attempt.stats {
            let (confirmed_version, version_confirmed) = if package.manifest.is_stm32wb_st_ble_ota()
            {
                let ok = stats.transport == STM32WB_ST_PROTOCOL_NAME;
                if !ok {
                    errors.push(format!(
                        "STM32WB ST OTA completed with unexpected transport {}.",
                        stats.transport
                    ));
                }
                (None, ok)
            } else {
                let expected_version = package.manifest.version.clone();
                let confirmed_version = confirm_firmware_ota_version_with(
                    &expected_version,
                    DEFAULT_CONFIRM_TIMEOUT,
                    || async {
                        let snapshot = crate::embedded_ble::firmware_ota_device_snapshot();
                        firmware_ota_snapshot_version(snapshot.firmware_version.as_deref())
                    },
                )
                .await;
                let version_confirmed = confirmed_version
                    .as_deref()
                    .is_some_and(|version| firmware_ota_versions_match(version, &expected_version));
                if !version_confirmed {
                    errors.push(
                            confirmed_version
                                .as_ref()
                                .map(|version| {
                                    format!(
                                        "Device reported firmware {version}, not {expected_version} after OTA reboot window."
                                    )
                                })
                                .unwrap_or_else(|| {
                                    "Device firmware version was not confirmed after the OTA reboot window."
                                        .to_string()
                                }),
                        );
                }
                (confirmed_version, version_confirmed)
            };
            if !version_confirmed && package.manifest.is_stm32wb_st_ble_ota() {
                errors.push(
                    "STM32WB ST OTA did not return the expected reboot confirmation.".to_string(),
                );
            }
            transfer = Some(FirmwareOtaHeadlessTransfer {
                bytes_transferred: stats.bytes_transferred,
                transport: stats.transport.to_string(),
                confirmed_version,
                version_confirmed,
            });
        }
    } else if options.preflight_only {
        let snapshot = firmware_ota_device_snapshot_for_manifest(&package.manifest);
        let blockers = preflight_blockers(&package.manifest, &snapshot, options.recording_active);
        if !blockers.is_empty() {
            errors.extend(blockers.iter().cloned());
        }
        preflight = Some(headless_preflight_from_snapshot(
            &options, snapshot, blockers,
        ));
    }

    FirmwareOtaHeadlessReport {
        status: if errors.is_empty() { "PASS" } else { "FAIL" }.to_string(),
        mode,
        manifest_path: package.manifest_path,
        firmware_path: package.firmware_path,
        manifest: Some(package.manifest),
        firmware_sha256: Some(package.firmware_sha256),
        package_valid: true,
        preflight,
        transfer,
        errors,
        warnings,
    }
}

fn firmware_ota_device_snapshot_for_manifest(
    manifest: &FirmwareOtaManifest,
) -> crate::embedded_ble::FirmwareOtaDeviceSnapshot {
    if manifest.is_stm32wb_st_ble_ota() {
        crate::embedded_ble::stm32wb_st_ota_device_snapshot()
    } else {
        crate::embedded_ble::firmware_ota_device_snapshot()
    }
}

struct HeadlessTransferAttempt {
    preflight: FirmwareOtaHeadlessPreflight,
    stats: Option<crate::embedded_ble::FirmwareOtaTransferStats>,
    errors: Vec<String>,
}

fn run_transfer_preflight_and_write(
    package: &FirmwareOtaPackage,
    options: &FirmwareOtaHeadlessOptions,
) -> HeadlessTransferAttempt {
    if package.manifest.is_stm32wb_st_ble_ota() {
        return run_stm32wb_st_transfer_preflight_and_write(package, options);
    }

    let prepared = match crate::embedded_ble::prepare_firmware_ota_transfer() {
        Ok(prepared) => prepared,
        Err(err) => {
            let snapshot = disconnected_ota_snapshot(err);
            let blockers =
                preflight_blockers(&package.manifest, &snapshot, options.recording_active);
            return HeadlessTransferAttempt {
                preflight: headless_preflight_from_snapshot(options, snapshot, blockers.clone()),
                stats: None,
                errors: blockers,
            };
        }
    };

    let snapshot = prepared.snapshot().clone();
    let blockers = preflight_blockers(&package.manifest, &snapshot, options.recording_active);
    let preflight = headless_preflight_from_snapshot(options, snapshot, blockers.clone());
    if !blockers.is_empty() {
        return HeadlessTransferAttempt {
            preflight,
            stats: None,
            errors: blockers,
        };
    }

    match prepared.transfer(
        &package.manifest.version,
        &package.firmware_sha256,
        &package.firmware_bytes,
        package.manifest.gatt_chunk_bytes as usize,
        None,
    ) {
        Ok(stats) => HeadlessTransferAttempt {
            preflight,
            stats: Some(stats),
            errors: Vec::new(),
        },
        Err(err) => HeadlessTransferAttempt {
            preflight,
            stats: None,
            errors: vec![err],
        },
    }
}

fn run_stm32wb_st_transfer_preflight_and_write(
    package: &FirmwareOtaPackage,
    options: &FirmwareOtaHeadlessOptions,
) -> HeadlessTransferAttempt {
    let prepared = match crate::embedded_ble::prepare_stm32wb_st_ota_transfer() {
        Ok(prepared) => prepared,
        Err(err) => {
            let snapshot = disconnected_ota_snapshot(err);
            let blockers =
                preflight_blockers(&package.manifest, &snapshot, options.recording_active);
            return HeadlessTransferAttempt {
                preflight: headless_preflight_from_snapshot(options, snapshot, blockers.clone()),
                stats: None,
                errors: blockers,
            };
        }
    };

    let snapshot = prepared.snapshot().clone();
    let blockers = preflight_blockers(&package.manifest, &snapshot, options.recording_active);
    let preflight = headless_preflight_from_snapshot(options, snapshot, blockers.clone());
    if !blockers.is_empty() {
        return HeadlessTransferAttempt {
            preflight,
            stats: None,
            errors: blockers,
        };
    }

    match prepared.transfer(
        &package.firmware_bytes,
        package.manifest.gatt_chunk_bytes as usize,
        None,
    ) {
        Ok(stats) => HeadlessTransferAttempt {
            preflight,
            stats: Some(stats),
            errors: Vec::new(),
        },
        Err(err) => HeadlessTransferAttempt {
            preflight,
            stats: None,
            errors: vec![err],
        },
    }
}

fn disconnected_ota_snapshot(detail: String) -> crate::embedded_ble::FirmwareOtaDeviceSnapshot {
    crate::embedded_ble::FirmwareOtaDeviceSnapshot {
        connected: false,
        hardware_revision: None,
        firmware_version: None,
        capabilities: Vec::new(),
        battery_percent: None,
        usb_powered: None,
        detail: Some(detail),
    }
}

fn headless_preflight_from_snapshot(
    options: &FirmwareOtaHeadlessOptions,
    snapshot: crate::embedded_ble::FirmwareOtaDeviceSnapshot,
    blockers: Vec<String>,
) -> FirmwareOtaHeadlessPreflight {
    FirmwareOtaHeadlessPreflight {
        recording_active: options.recording_active,
        dictation_phase: options.dictation_phase.clone(),
        connected: snapshot.connected,
        hardware_revision: snapshot.hardware_revision,
        firmware_version: snapshot.firmware_version,
        capabilities: snapshot.capabilities,
        battery_percent: snapshot.battery_percent,
        usb_powered: snapshot.usb_powered,
        detail: snapshot.detail,
        blockers,
    }
}

fn mode_name(options: &FirmwareOtaHeadlessOptions) -> &'static str {
    if options.transfer {
        "transfer"
    } else if options.preflight_only {
        "preflight"
    } else {
        "package"
    }
}

fn preflight_blockers(
    manifest: &FirmwareOtaManifest,
    snapshot: &crate::embedded_ble::FirmwareOtaDeviceSnapshot,
    recording_active: bool,
) -> Vec<String> {
    let mut blockers = Vec::new();
    if recording_active {
        blockers.push("Recording or dictation is still active; stop it before OTA.".to_string());
    }
    if !snapshot.connected {
        blockers.push(
            snapshot
                .detail
                .as_ref()
                .map(|detail| format!("Device is not ready for OTA: {detail}"))
                .unwrap_or_else(|| "Device is not connected.".to_string()),
        );
    }
    match snapshot.hardware_revision.as_deref() {
        Some(hardware) if hardware != manifest.hardware_revision => blockers.push(format!(
            "Hardware revision mismatch: device={hardware}, package={}.",
            manifest.hardware_revision
        )),
        None if snapshot.connected && manifest.is_listener_ble_ota() => {
            blockers.push("Device hardware revision is unknown.".to_string())
        }
        _ => {}
    }
    if manifest.is_listener_ble_ota()
        && !snapshot
            .capabilities
            .iter()
            .any(|item| item == FIRMWARE_CAPABILITY)
    {
        blockers.push("Connected firmware does not advertise OTA support.".to_string());
    }
    if manifest.is_stm32wb_st_ble_ota()
        && !snapshot
            .capabilities
            .iter()
            .any(|item| item == STM32WB_ST_FIRMWARE_CAPABILITY)
    {
        blockers
            .push("Connected STM32WB firmware does not advertise ST BLE OTA support.".to_string());
    }
    if manifest.is_listener_ble_ota()
        && snapshot.usb_powered != Some(true)
        && snapshot.battery_percent.is_none()
    {
        blockers.push("Power state is unknown; connect USB power before OTA.".to_string());
    }
    blockers
}

pub fn validate_package(
    manifest_text: &str,
    firmware_bytes: &[u8],
    context: &FirmwareOtaValidationContext,
) -> FirmwareOtaPackageValidation {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    let value = match serde_json::from_str::<Value>(manifest_text) {
        Ok(value) => value,
        Err(err) => {
            return FirmwareOtaPackageValidation {
                ok: false,
                manifest: None,
                firmware_sha256: None,
                errors: vec![format!("ota_manifest.json must be valid JSON: {err}")],
                warnings,
            };
        }
    };

    let manifest = match parse_manifest(&value) {
        Ok(manifest) => manifest,
        Err(err) => {
            return FirmwareOtaPackageValidation {
                ok: false,
                manifest: None,
                firmware_sha256: None,
                errors: vec![err],
                warnings,
            };
        }
    };

    if manifest.file_name != OTA_FILE_NAME {
        errors.push(format!("Package must include {OTA_FILE_NAME}."));
    }
    if firmware_bytes.len() as u64 != manifest.file_size_bytes {
        errors.push(format!(
            "Firmware size mismatch: manifest={}, actual={}.",
            manifest.file_size_bytes,
            firmware_bytes.len()
        ));
    }

    let firmware_sha256 = sha256_hex(firmware_bytes);
    if firmware_sha256 != manifest.file_sha256 {
        errors.push("Firmware SHA256 does not match ota_manifest.json.".to_string());
    }
    if compare_versionish(&context.desktop_version, &manifest.min_desktop_version) < 0 {
        errors.push(format!(
            "Listener Type {} is older than required {}.",
            context.desktop_version, manifest.min_desktop_version
        ));
    }
    if manifest.is_listener_ble_ota() && manifest.version.len() > OTA_MAX_VERSION_CHARS {
        errors.push(format!(
            "Firmware version is too long for BLE OTA control; expected <= {OTA_MAX_VERSION_CHARS} characters."
        ));
    }
    if manifest.is_listener_ble_ota()
        && manifest.hardware_revision != context.expected_hardware_revision
    {
        errors.push(format!(
            "Hardware revision mismatch: package={}, expected={}.",
            manifest.hardware_revision, context.expected_hardware_revision
        ));
    }
    if let Some(current) = context.current_firmware_version.as_deref() {
        if compare_versionish(&manifest.version, current) <= 0 {
            warnings.push(
                "Package version is not newer than the connected firmware version.".to_string(),
            );
        }
    }

    FirmwareOtaPackageValidation {
        ok: errors.is_empty(),
        manifest: Some(manifest),
        firmware_sha256: Some(firmware_sha256),
        errors,
        warnings,
    }
}

pub fn load_package(
    manifest_path: &Path,
    firmware_path: &Path,
    context: &FirmwareOtaValidationContext,
) -> Result<FirmwareOtaPackage, FirmwareOtaPackageValidation> {
    let manifest_text = match fs::read_to_string(manifest_path) {
        Ok(text) => text,
        Err(err) => {
            return Err(FirmwareOtaPackageValidation {
                ok: false,
                manifest: None,
                firmware_sha256: None,
                errors: vec![format!(
                    "Failed to read ota_manifest.json at {}: {err}",
                    manifest_path.display()
                )],
                warnings: Vec::new(),
            });
        }
    };
    let firmware_bytes = match fs::read(firmware_path) {
        Ok(bytes) => bytes,
        Err(err) => {
            return Err(FirmwareOtaPackageValidation {
                ok: false,
                manifest: None,
                firmware_sha256: None,
                errors: vec![format!(
                    "Failed to read firmware_ota.bin at {}: {err}",
                    firmware_path.display()
                )],
                warnings: Vec::new(),
            });
        }
    };
    let validation = validate_package(&manifest_text, &firmware_bytes, context);
    if !validation.ok {
        return Err(validation);
    }
    Ok(FirmwareOtaPackage {
        manifest_path: manifest_path.to_path_buf(),
        firmware_path: firmware_path.to_path_buf(),
        manifest: validation
            .manifest
            .clone()
            .expect("valid package includes parsed manifest"),
        firmware_bytes,
        firmware_sha256: validation
            .firmware_sha256
            .clone()
            .expect("valid package includes firmware hash"),
        warnings: validation.warnings,
    })
}

pub async fn confirm_firmware_ota_version_with<F, Fut>(
    expected_version: &str,
    timeout: Duration,
    mut snapshot: F,
) -> Option<String>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<String>>,
{
    if normalize_firmware_ota_version(expected_version).is_empty() || expected_version == "unknown"
    {
        return None;
    }

    let deadline = Instant::now() + timeout;
    let mut last_seen_version = None;
    loop {
        if let Some(version) = snapshot().await {
            if firmware_ota_versions_match(&version, expected_version) {
                return Some(version);
            }
            last_seen_version = Some(version);
        }

        if Instant::now() >= deadline {
            return last_seen_version;
        }
        tokio::time::sleep(CONFIRM_INTERVAL).await;
    }
}

pub fn firmware_ota_versions_match(confirmed: &str, expected: &str) -> bool {
    let confirmed = normalize_firmware_ota_version(confirmed);
    let expected = normalize_firmware_ota_version(expected);
    !confirmed.is_empty() && !expected.is_empty() && confirmed == expected
}

pub fn firmware_ota_snapshot_version(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

pub fn compare_versionish(left: &str, right: &str) -> i32 {
    let a = version_parts(left);
    let b = version_parts(right);
    let len = a.len().max(b.len());
    for index in 0..len {
        let delta = a.get(index).copied().unwrap_or(0) - b.get(index).copied().unwrap_or(0);
        if delta != 0 {
            return if delta > 0 { 1 } else { -1 };
        }
    }
    0
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    // Small, dependency-free SHA-256 implementation used to keep the GUI backend
    // and headless helper aligned without adding a new src-tauri crate dependency.
    const H0: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    let mut h = H0;
    let bit_len = (bytes.len() as u64) * 8;
    let mut data = bytes.to_vec();
    data.push(0x80);
    while (data.len() % 64) != 56 {
        data.push(0);
    }
    data.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in data.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (index, word) in w.iter_mut().take(16).enumerate() {
            let offset = index * 4;
            *word = u32::from_be_bytes([
                chunk[offset],
                chunk[offset + 1],
                chunk[offset + 2],
                chunk[offset + 3],
            ]);
        }
        for index in 16..64 {
            let s0 = w[index - 15].rotate_right(7)
                ^ w[index - 15].rotate_right(18)
                ^ (w[index - 15] >> 3);
            let s1 = w[index - 2].rotate_right(17)
                ^ w[index - 2].rotate_right(19)
                ^ (w[index - 2] >> 10);
            w[index] = w[index - 16]
                .wrapping_add(s0)
                .wrapping_add(w[index - 7])
                .wrapping_add(s1);
        }

        let mut a = h[0];
        let mut b = h[1];
        let mut c = h[2];
        let mut d = h[3];
        let mut e = h[4];
        let mut f = h[5];
        let mut g = h[6];
        let mut hh = h[7];

        for index in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[index])
                .wrapping_add(w[index]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }

    h.iter().map(|word| format!("{word:08x}")).collect()
}

fn parse_manifest(value: &Value) -> Result<FirmwareOtaManifest, String> {
    let schema_version = require_u64(
        value
            .get("schema_version")
            .or_else(|| value.get("schemaVersion")),
        "schema_version",
    )?;
    match schema_version {
        1 => parse_manifest_v1(value, schema_version),
        2 => parse_manifest_v2(value, schema_version),
        other => Err(format!("Unsupported OTA manifest schema_version {other}.")),
    }
}

fn parse_manifest_v1(value: &Value, schema_version: u64) -> Result<FirmwareOtaManifest, String> {
    let protocol = require_object(value.get("protocol"), "protocol")?;
    let file = require_object(value.get("file"), "file")?;
    let rollback = require_object(value.get("rollback"), "rollback")?;
    let recovery = require_object(value.get("recovery"), "recovery")?;
    let gatt = protocol.get("gatt").and_then(Value::as_object);

    let manifest = FirmwareOtaManifest {
        schema_version,
        package_type: require_string(
            value
                .get("package_type")
                .or_else(|| value.get("packageType")),
            "package_type",
        )?,
        project: require_string(value.get("project"), "project")?,
        version: require_string(value.get("version"), "version")?,
        protocol_name: require_string(protocol.get("name"), "protocol.name")?,
        protocol_version: require_u64(protocol.get("version"), "protocol.version")?,
        hardware_revision: require_string(
            value
                .get("hardware_revision")
                .or_else(|| value.get("hardwareRevision")),
            "hardware_revision",
        )?,
        min_desktop_version: require_string(
            value
                .get("min_desktop_version")
                .or_else(|| value.get("minDesktopVersion")),
            "min_desktop_version",
        )?,
        channel: require_channel(value.get("channel"))?,
        file_name: require_string(file.get("name"), "file.name")?,
        file_size_bytes: require_u64(
            file.get("size_bytes").or_else(|| file.get("sizeBytes")),
            "file.size_bytes",
        )?,
        file_sha256: require_string(file.get("sha256"), "file.sha256")?.to_ascii_lowercase(),
        firmware_capability: require_string(
            protocol
                .get("firmware_capability")
                .or_else(|| protocol.get("firmwareCapability")),
            "protocol.firmware_capability",
        )?,
        gatt_service_uuid: optional_gatt_string(
            gatt,
            "service_uuid",
            "serviceUuid",
            OTA_SERVICE_UUID,
        )?,
        gatt_control_uuid: optional_gatt_string(
            gatt,
            "control_uuid",
            "controlUuid",
            OTA_CONTROL_UUID,
        )?,
        gatt_data_uuid: optional_gatt_string(gatt, "data_uuid", "dataUuid", OTA_DATA_UUID)?,
        gatt_confirm_uuid: optional_gatt_optional_string(gatt, "confirm_uuid", "confirmUuid")?,
        gatt_chunk_bytes: optional_gatt_u64(gatt, "chunk_bytes", "chunkBytes", OTA_CHUNK_BYTES)?,
        rollback_instructions: require_instructions(
            rollback.get("instructions"),
            "rollback.instructions",
        )?,
        recovery_instructions: require_instructions(
            recovery.get("instructions"),
            "recovery.instructions",
        )?,
    };
    validate_normalized_manifest(manifest)
}

fn parse_manifest_v2(value: &Value, schema_version: u64) -> Result<FirmwareOtaManifest, String> {
    let firmware = require_object(value.get("firmware"), "firmware")?;
    let requirements = require_object(value.get("requirements"), "requirements")?;
    let ble_identity = require_object(
        value
            .get("ble_identity")
            .or_else(|| value.get("bleIdentity")),
        "ble_identity",
    )?;
    let dis = require_object(ble_identity.get("dis"), "ble_identity.dis")?;
    let rollback = require_object(value.get("rollback"), "rollback")?;
    let recovery = require_object(value.get("recovery"), "recovery")?;

    require_string(
        value
            .get("created_at_utc")
            .or_else(|| value.get("createdAtUtc")),
        "created_at_utc",
    )?;
    require_string(
        firmware
            .get("git_commit")
            .or_else(|| firmware.get("gitCommit")),
        "firmware.git_commit",
    )?;
    require_bool(
        firmware
            .get("git_dirty")
            .or_else(|| firmware.get("gitDirty")),
        "firmware.git_dirty",
    )?;
    require_string(firmware.get("target"), "firmware.target")?;
    require_string(ble_identity.get("name"), "ble_identity.name")?;
    require_string(ble_identity.get("appearance"), "ble_identity.appearance")?;
    require_string(dis.get("model"), "ble_identity.dis.model")?;
    require_string(
        dis.get("hardware_revision")
            .or_else(|| dis.get("hardwareRevision")),
        "ble_identity.dis.hardware_revision",
    )?;
    require_string(
        dis.get("firmware_revision")
            .or_else(|| dis.get("firmwareRevision")),
        "ble_identity.dis.firmware_revision",
    )?;

    if !require_bool(rollback.get("supported"), "rollback.supported")? {
        return Err("rollback.supported must be true.".to_string());
    }
    let rollback_method = require_string(rollback.get("method"), "rollback.method")?;
    if rollback_method != "esp_idf_bootloader_rollback" {
        return Err(format!("Unsupported rollback.method {rollback_method}."));
    }
    let factory_reflash = require_string(
        recovery
            .get("factory_reflash")
            .or_else(|| recovery.get("factoryReflash")),
        "recovery.factory_reflash",
    )?;
    let serial_commands = require_string(
        recovery
            .get("serial_commands")
            .or_else(|| recovery.get("serialCommands")),
        "recovery.serial_commands",
    )?;

    let manifest = FirmwareOtaManifest {
        schema_version,
        package_type: "listener-firmware-ota".to_string(),
        project: require_string(firmware.get("project"), "firmware.project")?,
        version: require_string(firmware.get("version"), "firmware.version")?,
        protocol_name: PROTOCOL_NAME.to_string(),
        protocol_version: require_u64(
            requirements
                .get("protocol_version")
                .or_else(|| requirements.get("protocolVersion")),
            "requirements.protocol_version",
        )?,
        hardware_revision: require_string(
            requirements
                .get("hardware_revision")
                .or_else(|| requirements.get("hardwareRevision")),
            "requirements.hardware_revision",
        )?,
        min_desktop_version: require_string(
            requirements
                .get("min_desktop_version")
                .or_else(|| requirements.get("minDesktopVersion")),
            "requirements.min_desktop_version",
        )?,
        channel: require_channel(value.get("channel"))?,
        file_name: require_string(firmware.get("file"), "firmware.file")?,
        file_size_bytes: require_u64(
            firmware
                .get("size_bytes")
                .or_else(|| firmware.get("sizeBytes")),
            "firmware.size_bytes",
        )?,
        file_sha256: require_string(firmware.get("sha256"), "firmware.sha256")?
            .to_ascii_lowercase(),
        firmware_capability: FIRMWARE_CAPABILITY.to_string(),
        gatt_service_uuid: OTA_SERVICE_UUID.to_string(),
        gatt_control_uuid: OTA_CONTROL_UUID.to_string(),
        gatt_data_uuid: OTA_DATA_UUID.to_string(),
        gatt_confirm_uuid: None,
        gatt_chunk_bytes: optional_u64(
            requirements
                .get("gatt_chunk_bytes")
                .or_else(|| requirements.get("gattChunkBytes")),
            OTA_CHUNK_BYTES,
        )?,
        rollback_instructions: require_instructions(
            rollback.get("instructions"),
            "rollback.instructions",
        )?,
        recovery_instructions: vec![factory_reflash, serial_commands],
    };
    validate_normalized_manifest(manifest)
}

pub fn validate_normalized_manifest(
    manifest: FirmwareOtaManifest,
) -> Result<FirmwareOtaManifest, String> {
    if manifest.is_listener_ble_ota() {
        if manifest.package_type != "listener-firmware-ota" {
            return Err(
                "ota_manifest.json package_type must be listener-firmware-ota.".to_string(),
            );
        }
    } else if manifest.is_stm32wb_st_ble_ota() {
        if manifest.package_type != "companion-firmware-ota" {
            return Err(
                "Companion STM32WB OTA package_type must be companion-firmware-ota.".to_string(),
            );
        }
    } else {
        return Err(format!(
            "Unsupported OTA protocol {}.",
            manifest.protocol_name
        ));
    }
    if manifest.protocol_version < 1 {
        return Err("OTA protocol.version must be >= 1.".to_string());
    }
    if manifest.is_listener_ble_ota() {
        if manifest.firmware_capability != FIRMWARE_CAPABILITY {
            return Err("OTA package requires unsupported firmware capability.".to_string());
        }
        if !uuid_eq(&manifest.gatt_service_uuid, OTA_SERVICE_UUID)
            || !uuid_eq(&manifest.gatt_control_uuid, OTA_CONTROL_UUID)
            || !uuid_eq(&manifest.gatt_data_uuid, OTA_DATA_UUID)
        {
            return Err("OTA package uses an unsupported BLE OTA GATT boundary.".to_string());
        }
        if manifest.gatt_chunk_bytes != OTA_MAX_CHUNK_BYTES {
            return Err(format!(
                "OTA package uses unsupported BLE OTA chunk size {}; supported value is {OTA_MAX_CHUNK_BYTES}.",
                manifest.gatt_chunk_bytes
            ));
        }
    } else if manifest.is_stm32wb_st_ble_ota() {
        if manifest.project != "Companion-Firmware" {
            return Err("Companion STM32WB OTA project must be Companion-Firmware.".to_string());
        }
        if manifest.firmware_capability != STM32WB_ST_FIRMWARE_CAPABILITY {
            return Err(
                "Companion STM32WB OTA package requires unsupported firmware capability."
                    .to_string(),
            );
        }
        if !uuid_eq(&manifest.gatt_service_uuid, STM32WB_ST_OTA_SERVICE_UUID)
            || !uuid_eq(&manifest.gatt_control_uuid, STM32WB_ST_OTA_CONTROL_UUID)
            || !uuid_eq(&manifest.gatt_data_uuid, STM32WB_ST_OTA_DATA_UUID)
            || manifest
                .gatt_confirm_uuid
                .as_deref()
                .map_or(true, |value| !uuid_eq(value, STM32WB_ST_OTA_CONFIRM_UUID))
        {
            return Err(
                "Companion STM32WB OTA package uses an unsupported ST GATT boundary.".to_string(),
            );
        }
        if manifest.gatt_chunk_bytes != STM32WB_ST_OTA_CHUNK_BYTES {
            return Err(format!(
                "Companion STM32WB OTA chunk size must be {STM32WB_ST_OTA_CHUNK_BYTES} bytes, got {}.",
                manifest.gatt_chunk_bytes
            ));
        }
    }
    if manifest.file_size_bytes == 0 {
        return Err("file.size_bytes must be greater than zero.".to_string());
    }
    if !is_lower_sha256(&manifest.file_sha256) {
        return Err("file.sha256 must be lowercase SHA256 hex.".to_string());
    }
    Ok(manifest)
}

fn require_object<'a>(
    value: Option<&'a Value>,
    field: &str,
) -> Result<&'a serde_json::Map<String, Value>, String> {
    value
        .and_then(Value::as_object)
        .ok_or_else(|| format!("{field} must be an object."))
}

fn require_string(value: Option<&Value>, field: &str) -> Result<String, String> {
    let value = value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("{field} must be a non-empty string."))?;
    Ok(value.to_string())
}

fn require_u64(value: Option<&Value>, field: &str) -> Result<u64, String> {
    value
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("{field} must be a number."))
}

fn optional_u64(value: Option<&Value>, default_value: u64) -> Result<u64, String> {
    match value {
        Some(value) => value
            .as_u64()
            .ok_or_else(|| "optional numeric field must be a number.".to_string()),
        None => Ok(default_value),
    }
}

fn require_bool(value: Option<&Value>, field: &str) -> Result<bool, String> {
    value
        .and_then(Value::as_bool)
        .ok_or_else(|| format!("{field} must be a boolean."))
}

fn require_channel(value: Option<&Value>) -> Result<String, String> {
    let channel = require_string(value, "channel")?;
    match channel.as_str() {
        "stable" | "development" => Ok(channel),
        _ => Err("channel must be stable or development.".to_string()),
    }
}

fn uuid_eq(left: &str, right: &str) -> bool {
    left.eq_ignore_ascii_case(right)
}

fn require_instructions(value: Option<&Value>, field: &str) -> Result<Vec<String>, String> {
    if let Some(text) = value.and_then(Value::as_str) {
        let text = text.trim();
        if !text.is_empty() {
            return Ok(vec![text.to_string()]);
        }
    }
    let Some(items) = value.and_then(Value::as_array) else {
        return Err(format!("{field} must be an array."));
    };
    let strings: Vec<String> = items
        .iter()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect();
    if strings.is_empty() {
        return Err(format!("{field} must contain at least one instruction."));
    }
    Ok(strings)
}

fn optional_gatt_string(
    gatt: Option<&serde_json::Map<String, Value>>,
    snake: &str,
    camel: &str,
    default_value: &str,
) -> Result<String, String> {
    match gatt {
        Some(gatt) => require_string(
            gatt.get(snake).or_else(|| gatt.get(camel)),
            &format!("protocol.gatt.{snake}"),
        ),
        None => Ok(default_value.to_string()),
    }
}

fn optional_gatt_optional_string(
    gatt: Option<&serde_json::Map<String, Value>>,
    snake: &str,
    camel: &str,
) -> Result<Option<String>, String> {
    let Some(gatt) = gatt else {
        return Ok(None);
    };
    let value = gatt.get(snake).or_else(|| gatt.get(camel));
    match value {
        Some(_) => require_string(value, &format!("protocol.gatt.{snake}")).map(Some),
        None => Ok(None),
    }
}

fn optional_gatt_u64(
    gatt: Option<&serde_json::Map<String, Value>>,
    snake: &str,
    camel: &str,
    default_value: u64,
) -> Result<u64, String> {
    match gatt {
        Some(gatt) => require_u64(
            gatt.get(snake).or_else(|| gatt.get(camel)),
            &format!("protocol.gatt.{snake}"),
        ),
        None => Ok(default_value),
    }
}

fn is_lower_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn normalize_firmware_ota_version(value: &str) -> String {
    value.trim().trim_start_matches('v').to_ascii_lowercase()
}

fn version_parts(value: &str) -> Vec<i32> {
    let normalized = value.trim().trim_start_matches('v');
    let mut best = String::new();
    let mut current = String::new();
    for ch in normalized.chars() {
        if ch.is_ascii_digit() || (ch == '.' && current.chars().any(|item| item.is_ascii_digit())) {
            current.push(ch);
        } else if !current.is_empty() {
            if current.len() > best.len() {
                best = current;
            }
            current = String::new();
        }
    }
    if current.len() > best.len() {
        best = current;
    }
    if best.is_empty() {
        return vec![0];
    }
    best.split('.')
        .filter_map(|part| part.parse::<i32>().ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIRMWARE_BYTES: &[u8] = &[0xe9, 1, 2, 3, 4, 5];
    const FIRMWARE_SHA256: &str =
        "6d3841935f58db1c3efa67022f2d770184be6fdef93c087bca10c30e70157e84";

    fn context() -> FirmwareOtaValidationContext {
        FirmwareOtaValidationContext {
            desktop_version: "1.0.0".to_string(),
            expected_hardware_revision: "keyboard-v1".to_string(),
            current_firmware_version: None,
        }
    }

    fn manifest_v2(extra: &str) -> String {
        format!(
            r#"{{
  "schema_version": 2,
  "created_at_utc": "2026-05-26T00:00:00Z",
  "channel": "development",
  "firmware": {{
    "project": "voice-keyboard-firmware",
    "version": "1.2.0",
    "git_commit": "{git_commit}",
    "git_dirty": false,
    "target": "esp32s3",
    "file": "firmware_ota.bin",
    "size_bytes": 6,
    "sha256": "{FIRMWARE_SHA256}"
  }},
  "requirements": {{
    "hardware_revision": "keyboard-v1",
    "protocol_version": 1,
    "min_desktop_version": "1.0.0"
  }},
  "ble_identity": {{
    "name": "listener",
    "appearance": "0x03C1",
    "dis": {{
      "model": "keyboard-v1",
      "hardware_revision": "esp32s3-devkit",
      "firmware_revision": "1.2.0"
    }}
  }},
  "rollback": {{
    "supported": true,
    "method": "esp_idf_bootloader_rollback",
    "instructions": "Rollback on failed pending verify."
  }},
  "recovery": {{
    "factory_reflash": "Use USB factory package.",
    "serial_commands": "~OTA:STATUS"
  }}
  {extra}
}}"#,
            git_commit = "a".repeat(40)
        )
    }

    fn stm32wb_manifest() -> FirmwareOtaManifest {
        FirmwareOtaManifest {
            schema_version: 2,
            package_type: "companion-firmware-ota".to_string(),
            project: "Companion-Firmware".to_string(),
            version: "3119c18-dirty".to_string(),
            protocol_name: STM32WB_ST_PROTOCOL_NAME.to_string(),
            protocol_version: 1,
            hardware_revision: "NUCLEO-WB55RG".to_string(),
            min_desktop_version: "1.0.0".to_string(),
            channel: "development".to_string(),
            file_name: OTA_FILE_NAME.to_string(),
            file_size_bytes: FIRMWARE_BYTES.len() as u64,
            file_sha256: FIRMWARE_SHA256.to_string(),
            firmware_capability: STM32WB_ST_FIRMWARE_CAPABILITY.to_string(),
            gatt_service_uuid: STM32WB_ST_OTA_SERVICE_UUID.to_string(),
            gatt_control_uuid: STM32WB_ST_OTA_CONTROL_UUID.to_string(),
            gatt_data_uuid: STM32WB_ST_OTA_DATA_UUID.to_string(),
            gatt_confirm_uuid: Some(STM32WB_ST_OTA_CONFIRM_UUID.to_string()),
            gatt_chunk_bytes: STM32WB_ST_OTA_CHUNK_BYTES,
            rollback_instructions: vec!["Re-run wired factory flash.".to_string()],
            recovery_instructions: vec!["Use ST-LINK wired package.".to_string()],
        }
    }

    fn stm32wb_loader_snapshot() -> crate::embedded_ble::FirmwareOtaDeviceSnapshot {
        crate::embedded_ble::FirmwareOtaDeviceSnapshot {
            connected: true,
            hardware_revision: Some("NUCLEO-WB55RG".to_string()),
            firmware_version: Some("companion OTA loader".to_string()),
            capabilities: vec![STM32WB_ST_FIRMWARE_CAPABILITY.to_string()],
            battery_percent: None,
            usb_powered: None,
            detail: None,
        }
    }

    #[test]
    fn validates_schema_v2_package() {
        let result = validate_package(&manifest_v2(""), FIRMWARE_BYTES, &context());

        assert!(result.ok, "{:?}", result.errors);
        assert_eq!(result.firmware_sha256.as_deref(), Some(FIRMWARE_SHA256));
        let manifest = result.manifest.unwrap();
        assert_eq!(manifest.version, "1.2.0");
        assert_eq!(manifest.gatt_chunk_bytes, OTA_MAX_CHUNK_BYTES);
    }

    #[test]
    fn schema_v2_accepts_required_gatt_chunk_size() {
        let result = validate_package(
            &manifest_v2(
                r#","requirements":{"hardware_revision":"keyboard-v1","protocol_version":1,"min_desktop_version":"1.0.0","gatt_chunk_bytes":500}"#,
            ),
            FIRMWARE_BYTES,
            &context(),
        );

        assert!(result.ok, "{:?}", result.errors);
        assert_eq!(
            result.manifest.unwrap().gatt_chunk_bytes,
            OTA_MAX_CHUNK_BYTES
        );
    }

    #[test]
    fn rejects_gatt_chunk_above_safe_limit() {
        let result = validate_package(
            &manifest_v2(
                r#","requirements":{"hardware_revision":"keyboard-v1","protocol_version":1,"min_desktop_version":"1.0.0","gatt_chunk_bytes":499}"#,
            ),
            FIRMWARE_BYTES,
            &context(),
        );

        assert!(!result.ok);
        assert!(result.errors.iter().any(|item| item.contains("chunk size")));
    }

    #[test]
    fn rejects_hash_and_size_mismatch() {
        let bad_manifest = manifest_v2(
            r#","firmware":{"project":"voice-keyboard-firmware","version":"1.2.0","git_commit":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","git_dirty":false,"target":"esp32s3","file":"firmware_ota.bin","size_bytes":7,"sha256":"0000000000000000000000000000000000000000000000000000000000000000"}"#,
        );
        let result = validate_package(&bad_manifest, FIRMWARE_BYTES, &context());

        assert!(!result.ok);
        assert!(result
            .errors
            .iter()
            .any(|item| item.contains("size mismatch")));
        assert!(result.errors.iter().any(|item| item.contains("SHA256")));
    }

    #[test]
    fn rejects_version_too_long_for_ble_ota_control() {
        let manifest = manifest_v2(
            r#","firmware":{"project":"voice-keyboard-firmware","version":"1.0.0-local-build-226-g99934ff-dirty","git_commit":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","git_dirty":true,"target":"esp32s3","file":"firmware_ota.bin","size_bytes":6,"sha256":"6d3841935f58db1c3efa67022f2d770184be6fdef93c087bca10c30e70157e84"}"#,
        );
        let result = validate_package(&manifest, FIRMWARE_BYTES, &context());

        assert!(!result.ok);
        assert!(result.errors.iter().any(|item| item.contains("too long")));
    }

    #[test]
    fn rejects_missing_recovery_field() {
        let manifest = manifest_v2(r#","recovery":{"factory_reflash":"Use USB factory package."}"#);
        let result = validate_package(&manifest, FIRMWARE_BYTES, &context());

        assert!(!result.ok);
        assert!(result
            .errors
            .iter()
            .any(|item| item.contains("recovery.serial_commands")));
    }

    #[test]
    fn version_helpers_match_ui_behavior() {
        assert_eq!(compare_versionish("v1.0.1", "1.0.0"), 1);
        assert_eq!(compare_versionish("1.0.0", "1.0.0"), 0);
        assert_eq!(compare_versionish("1.0.0", "1.0.1"), -1);
        assert!(firmware_ota_versions_match(" v1.2.0 ", "1.2.0"));
        assert!(firmware_ota_versions_match("1.2.0", "v1.2.0"));
        assert!(!firmware_ota_versions_match("1.2.0-dev", "1.2.0"));
        assert!(!firmware_ota_versions_match("", "1.2.0"));
    }

    #[test]
    fn stm32wb_st_loader_snapshot_passes_preflight() {
        let manifest = stm32wb_manifest();
        let snapshot = stm32wb_loader_snapshot();

        assert!(preflight_blockers(&manifest, &snapshot, false).is_empty());
    }

    #[test]
    fn stm32wb_st_manifest_requires_confirm_gatt_boundary() {
        let mut missing = stm32wb_manifest();
        missing.gatt_confirm_uuid = None;
        let missing_result = validate_normalized_manifest(missing);

        assert!(missing_result.is_err());
        assert!(missing_result
            .unwrap_err()
            .contains("unsupported ST GATT boundary"));

        let mut wrong = stm32wb_manifest();
        wrong.gatt_confirm_uuid = Some("8f7a7003-7b7d-4f3d-9d6f-6c2d1b7cffff".to_string());
        let wrong_result = validate_normalized_manifest(wrong);

        assert!(wrong_result.is_err());
        assert!(wrong_result
            .unwrap_err()
            .contains("unsupported ST GATT boundary"));
    }

    #[test]
    fn stm32wb_st_preflight_requires_st_capability() {
        let manifest = stm32wb_manifest();
        let mut snapshot = stm32wb_loader_snapshot();
        snapshot.capabilities.clear();

        let blockers = preflight_blockers(&manifest, &snapshot, false);

        assert!(blockers
            .iter()
            .any(|item| item.contains("ST BLE OTA support")));
    }

    #[test]
    fn sha256_matches_known_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(sha256_hex(FIRMWARE_BYTES), FIRMWARE_SHA256);
    }
}
