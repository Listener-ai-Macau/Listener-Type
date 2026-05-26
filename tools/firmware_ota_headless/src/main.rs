#![allow(dead_code)]

mod embedded_ble {
    use serde::Serialize;

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

    pub fn transfer_firmware_ota(
        _version: &str,
        _firmware_sha256: &str,
        _firmware_bytes: &[u8],
    ) -> Result<FirmwareOtaTransferStats, String> {
        Err(
            "Firmware OTA over Listener BLE is only available from the Tauri app on Windows."
                .to_string(),
        )
    }

    pub fn firmware_ota_device_snapshot() -> FirmwareOtaDeviceSnapshot {
        FirmwareOtaDeviceSnapshot {
            connected: false,
            hardware_revision: None,
            firmware_version: None,
            capabilities: Vec::new(),
            battery_percent: None,
            usb_powered: None,
            detail: Some(
                "Standalone headless helper only supports package validation; use listener-type --firmware-ota-transfer for BLE transfer."
                    .to_string(),
            ),
        }
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
  --desktop-version <version>    Listener Type version. Defaults to CARGO_PKG_VERSION.
  --hardware <revision>          Expected hardware revision. Defaults to keyboard-v1.
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
    let mut desktop_version = env!("CARGO_PKG_VERSION").to_string();
    let mut expected_hardware_revision = "keyboard-v1".to_string();
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

fn next_value(iter: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    iter.next()
        .filter(|value| !value.starts_with("--"))
        .ok_or_else(|| format!("{flag} requires a value"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_requires_manifest_and_firmware() {
        assert!(parse_args(vec![]).is_err());
        assert!(parse_args(vec!["--manifest".into(), "m.json".into()]).is_err());
    }

    #[test]
    fn parse_accepts_package_paths_and_options() {
        let args = parse_args(vec![
            "--manifest".into(),
            "ota_manifest.json".into(),
            "--firmware".into(),
            "firmware_ota.bin".into(),
            "--desktop-version".into(),
            "1.3.3".into(),
            "--hardware".into(),
            "keyboard-v1".into(),
            "--current-version".into(),
            "1.2.0".into(),
            "--preflight".into(),
        ])
        .expect("parse")
        .expect("args");

        assert_eq!(args.manifest_path, PathBuf::from("ota_manifest.json"));
        assert_eq!(args.firmware_path, PathBuf::from("firmware_ota.bin"));
        assert_eq!(args.desktop_version, "1.3.3");
        assert_eq!(args.expected_hardware_revision, "keyboard-v1");
        assert_eq!(args.current_firmware_version.as_deref(), Some("1.2.0"));
        assert!(args.preflight);
    }
}
