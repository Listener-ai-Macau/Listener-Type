#![allow(dead_code)]

mod embedded_ble {
    use serde::Serialize;
    use std::time::Duration;

    #[derive(Debug, Clone, PartialEq, Eq, Serialize)]
    #[serde(rename_all = "camelCase")]
    pub struct FirmwareOtaTransferStats {
        pub bytes_transferred: usize,
        pub chunks_sent: usize,
        pub transport: &'static str,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize)]
    #[serde(rename_all = "camelCase")]
    pub struct FirmwareOtaDeviceSnapshot {
        pub connected: bool,
        pub hardware_revision: Option<String>,
        pub firmware_version: Option<String>,
        pub capabilities: Vec<String>,
        pub battery_percent: Option<u8>,
        pub usb_powered: Option<bool>,
        pub detail: Option<String>,
    }

    pub struct PreparedListenerOtaV1Transfer {
        snapshot: FirmwareOtaDeviceSnapshot,
    }

    impl PreparedListenerOtaV1Transfer {
        pub fn snapshot(&self) -> &FirmwareOtaDeviceSnapshot {
            &self.snapshot
        }

        pub fn transfer(
            self,
            _firmware_sha256: &str,
            firmware_bytes: &[u8],
            manifest_chunk_bytes: usize,
            on_progress: Option<&dyn Fn(usize, usize)>,
        ) -> Result<FirmwareOtaTransferStats, String> {
            let _ = (firmware_bytes, manifest_chunk_bytes, on_progress);
            Err(
                "Denzic OTA v1 BLE transfer is only available from Listener Type on Windows."
                    .to_string(),
            )
        }
    }

    pub fn prepare_listener_ota_v1_transfer() -> Result<PreparedListenerOtaV1Transfer, String> {
        Err("Listener OTA v1 over BLE is only available from the Tauri app on Windows.".to_string())
    }

    pub fn listener_ota_v1_device_snapshot() -> FirmwareOtaDeviceSnapshot {
        FirmwareOtaDeviceSnapshot {
            connected: false,
            hardware_revision: None,
            firmware_version: None,
            capabilities: Vec::new(),
            battery_percent: None,
            usb_powered: None,
            detail: Some(
                "Standalone headless validation cannot access the Windows BLE adapter.".to_string(),
            ),
        }
    }

    pub fn listener_ota_v1_service_reachable_snapshot(
        _timeout: Duration,
    ) -> FirmwareOtaDeviceSnapshot {
        listener_ota_v1_device_snapshot()
    }

    pub fn listener_ota_v1_gatt_probe_snapshot(_timeout: Duration) -> FirmwareOtaDeviceSnapshot {
        listener_ota_v1_device_snapshot()
    }

    pub fn request_listener_ota_post_confirm_notify_fast_retry() {}

    pub fn probe_notify_subscription(_timeout: Duration) -> Result<(), String> {
        Err(
            "Standalone headless validation cannot probe Windows BLE notify subscription."
                .to_string(),
        )
    }
}

#[path = "../../../src-tauri/src/firmware_ota.rs"]
mod firmware_ota;

use std::env;
use std::path::PathBuf;

use firmware_ota::FirmwareOtaHeadlessOptions;

const USAGE: &str = "\
Usage:
  listener-firmware-ota-headless --manifest <ota_manifest.json> --firmware <firmware_ota.bin> [options]

Options:
  --manifest <file>              OTA manifest path.
  --firmware <file>              OTA binary path.
  --desktop-version <version>    Listener Type version. Defaults to the repository package.json version.
  --hardware <revision>          Expected hardware revision. Defaults to keyboard-v2-n16r8.
  --current-version <version>    Optional connected/current firmware version for warning checks.
  --preflight                    Include a device preflight snapshot. Standalone helper reports unsupported BLE.
  --json-out <file>              Write pretty JSON report to this file.
  --help                         Show this help.
";

#[derive(Debug)]
struct Args {
    manifest_path: PathBuf,
    firmware_path: PathBuf,
    desktop_version: String,
    expected_hardware_revision: String,
    current_firmware_version: Option<String>,
    preflight: bool,
    json_out: Option<PathBuf>,
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args = match parse_args(env::args().skip(1).collect()) {
        Ok(Some(args)) => args,
        Ok(None) => return,
        Err(err) => {
            eprintln!("{err}\n\n{USAGE}");
            std::process::exit(2);
        }
    };

    let options = FirmwareOtaHeadlessOptions {
        manifest_path: args.manifest_path,
        firmware_path: args.firmware_path,
        preflight_only: args.preflight,
        transfer: false,
        desktop_version: args.desktop_version,
        expected_hardware_revision: args.expected_hardware_revision,
        current_firmware_version: args.current_firmware_version,
        recording_active: false,
        dictation_phase: None,
    };
    let report = firmware_ota::run_headless(options).await;
    if let Some(path) = args.json_out {
        let pretty = serde_json::to_string_pretty(&report).expect("serialize pretty report");
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).expect("create report directory");
            }
        }
        std::fs::write(&path, pretty).expect("write report");
    }
    let compact = serde_json::to_string(&report).expect("serialize compact report");
    println!("firmware_ota_result_json={compact}");
    if report.status != "PASS" {
        std::process::exit(1);
    }
}

fn parse_args(args: Vec<String>) -> Result<Option<Args>, String> {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("{USAGE}");
        return Ok(None);
    }
    let mut manifest_path = None;
    let mut firmware_path = None;
    let mut desktop_version = repository_desktop_version()?;
    let mut expected_hardware_revision = "keyboard-v2-n16r8".to_string();
    let mut current_firmware_version = None;
    let mut preflight = false;
    let mut json_out = None;

    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--manifest" => manifest_path = Some(next_value(&mut iter, "--manifest")?.into()),
            "--firmware" => firmware_path = Some(next_value(&mut iter, "--firmware")?.into()),
            "--desktop-version" => desktop_version = next_value(&mut iter, "--desktop-version")?,
            "--hardware" => expected_hardware_revision = next_value(&mut iter, "--hardware")?,
            "--current-version" => {
                current_firmware_version = Some(next_value(&mut iter, "--current-version")?)
            }
            "--preflight" => preflight = true,
            "--json-out" => json_out = Some(next_value(&mut iter, "--json-out")?.into()),
            other => return Err(format!("unknown argument: {other}")),
        }
    }

    Ok(Some(Args {
        manifest_path: manifest_path.ok_or_else(|| "--manifest is required".to_string())?,
        firmware_path: firmware_path.ok_or_else(|| "--firmware is required".to_string())?,
        desktop_version,
        expected_hardware_revision,
        current_firmware_version,
        preflight,
        json_out,
    }))
}

fn repository_desktop_version() -> Result<String, String> {
    let package: serde_json::Value = serde_json::from_str(include_str!("../../../package.json"))
        .map_err(|err| format!("Listener Type package.json is invalid: {err}"))?;
    package
        .get("version")
        .and_then(serde_json::Value::as_str)
        .filter(|version| !version.trim().is_empty())
        .map(str::to_owned)
        .ok_or_else(|| "Listener Type package.json is missing version".to_string())
}

fn next_value(iter: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    iter.next()
        .filter(|value| !value.starts_with("--"))
        .ok_or_else(|| format!("{flag} requires a value"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    const FIRMWARE_BYTES: &[u8] = &[0xe9, 1, 2, 3, 4, 5];
    const FIRMWARE_SHA256: &str =
        "6d3841935f58db1c3efa67022f2d770184be6fdef93c087bca10c30e70157e84";

    fn validation_context() -> firmware_ota::FirmwareOtaValidationContext {
        firmware_ota::FirmwareOtaValidationContext {
            desktop_version: "1.0.0".to_string(),
            expected_hardware_revision: "keyboard-v2-n16r8".to_string(),
            current_firmware_version: None,
        }
    }

    fn manifest_v2() -> String {
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
    "hardware_revision": "keyboard-v2-n16r8",
    "protocol_version": 1,
    "min_desktop_version": "1.0.0"
  }},
  "protocol": {{
    "name": "denzic_ota_v1",
    "version": 1,
    "firmware_capability": "denzic_ota_v1",
    "gatt": {{
      "service_uuid": "{service_uuid}",
      "control_uuid": "{control_uuid}",
      "data_uuid": "{data_uuid}",
      "status_uuid": "{status_uuid}",
      "chunk_bytes": 500
    }}
  }},
  "ble_identity": {{
    "name": "listener",
    "appearance": "0x03C1",
    "dis": {{
      "model": "keyboard-v2",
      "hardware_revision": "esp32s3-wroom-1-n16r8",
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
}}"#,
            git_commit = "a".repeat(40),
            service_uuid = firmware_ota::LISTENER_OTA_V1_SERVICE_UUID,
            control_uuid = firmware_ota::LISTENER_OTA_V1_CONTROL_UUID,
            data_uuid = firmware_ota::LISTENER_OTA_V1_DATA_UUID,
            status_uuid = firmware_ota::LISTENER_OTA_V1_STATUS_UUID,
        )
    }

    fn temp_package_dir(test_name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock after unix epoch")
            .as_nanos();
        let path = env::temp_dir().join(format!(
            "listener_firmware_ota_headless_{test_name}_{}_{}",
            std::process::id(),
            unique
        ));
        fs::create_dir_all(&path).expect("create temp package dir");
        path
    }

    fn write_valid_package(test_name: &str) -> (PathBuf, PathBuf, PathBuf) {
        let dir = temp_package_dir(test_name);
        let manifest_path = dir.join("ota_manifest.json");
        let firmware_path = dir.join("firmware_ota.bin");
        fs::write(&manifest_path, manifest_v2()).expect("write manifest");
        fs::write(&firmware_path, FIRMWARE_BYTES).expect("write firmware");
        (dir, manifest_path, firmware_path)
    }

    #[test]
    fn parse_requires_manifest_and_firmware() {
        assert!(parse_args(vec![]).is_err());
        assert!(parse_args(vec!["--manifest".into(), "m.json".into()]).is_err());
    }

    #[test]
    fn parse_defaults_to_repository_desktop_version() {
        let args = parse_args(vec![
            "--manifest".into(),
            "ota_manifest.json".into(),
            "--firmware".into(),
            "firmware_ota.bin".into(),
        ])
        .expect("parse")
        .expect("args");

        assert_eq!(args.desktop_version, repository_desktop_version().unwrap());
    }

    #[test]
    fn parse_accepts_package_paths_and_options() {
        let args = parse_args(vec![
            "--manifest".into(),
            "ota_manifest.json".into(),
            "--firmware".into(),
            "firmware_ota.bin".into(),
            "--desktop-version".into(),
            "1.0.0".into(),
            "--hardware".into(),
            "keyboard-v2-n16r8".into(),
            "--current-version".into(),
            "1.2.0".into(),
            "--preflight".into(),
        ])
        .expect("parse")
        .expect("args");

        assert_eq!(args.manifest_path, PathBuf::from("ota_manifest.json"));
        assert_eq!(args.firmware_path, PathBuf::from("firmware_ota.bin"));
        assert_eq!(args.desktop_version, "1.0.0");
        assert_eq!(args.expected_hardware_revision, "keyboard-v2-n16r8");
        assert_eq!(args.current_firmware_version.as_deref(), Some("1.2.0"));
        assert!(args.preflight);
    }

    #[test]
    fn load_package_reports_missing_manifest_path() {
        let dir = temp_package_dir("missing_manifest");
        let manifest_path = dir.join("ota_manifest.json");
        let firmware_path = dir.join("firmware_ota.bin");
        fs::write(&firmware_path, FIRMWARE_BYTES).expect("write firmware");

        let result =
            firmware_ota::load_package(&manifest_path, &firmware_path, &validation_context());

        let validation = result.expect_err("missing manifest should fail package loading");
        assert!(!validation.ok);
        assert!(validation.manifest.is_none());
        assert!(validation.firmware_sha256.is_none());
        assert!(validation
            .errors
            .iter()
            .any(|item| item.contains("Failed to read ota_manifest.json")));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn load_package_reports_missing_firmware_binary_path() {
        let dir = temp_package_dir("missing_firmware");
        let manifest_path = dir.join("ota_manifest.json");
        let firmware_path = dir.join("firmware_ota.bin");
        fs::write(&manifest_path, manifest_v2()).expect("write manifest");

        let result =
            firmware_ota::load_package(&manifest_path, &firmware_path, &validation_context());

        let validation = result.expect_err("missing firmware should fail package loading");
        assert!(!validation.ok);
        assert!(validation.manifest.is_none());
        assert!(validation.firmware_sha256.is_none());
        assert!(validation
            .errors
            .iter()
            .any(|item| item.contains("Failed to read firmware_ota.bin")));
        let _ = fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn preflight_only_reports_no_hardware_fail_without_transfer() {
        let (dir, manifest_path, firmware_path) = write_valid_package("preflight_no_hardware");

        let report = firmware_ota::run_headless(FirmwareOtaHeadlessOptions {
            manifest_path,
            firmware_path,
            preflight_only: true,
            transfer: false,
            desktop_version: "1.0.0".to_string(),
            expected_hardware_revision: "keyboard-v2-n16r8".to_string(),
            current_firmware_version: None,
            recording_active: false,
            dictation_phase: Some("HeadlessTest".to_string()),
        })
        .await;

        assert_eq!(report.status, "FAIL");
        assert_eq!(report.mode, "preflight");
        assert!(report.package_valid);
        assert!(report.transfer.is_none());
        assert_eq!(report.firmware_sha256.as_deref(), Some(FIRMWARE_SHA256));
        assert!(report
            .errors
            .iter()
            .any(|item| item.contains("Device is not ready for OTA")));

        let preflight = report.preflight.expect("preflight report");
        assert!(!preflight.connected);
        assert_eq!(preflight.dictation_phase.as_deref(), Some("HeadlessTest"));
        assert!(preflight
            .detail
            .as_deref()
            .is_some_and(|item| item.contains("Standalone headless validation")));
        assert!(preflight
            .blockers
            .iter()
            .any(|item| item.contains("Device is not ready for OTA")));
        let _ = fs::remove_dir_all(dir);
    }
}
