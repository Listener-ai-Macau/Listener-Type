#!/usr/bin/env python3
"""Reject regressions that bypass Listener's BLE device-control transaction."""

from pathlib import Path
import sys


ROOT = Path(__file__).resolve().parents[1]


def read(relative: str) -> str:
    path = ROOT / relative
    if not path.is_file():
        raise RuntimeError(f"missing required file: {relative}")
    return path.read_text(encoding="utf-8")


def require(text: str, needle: str, label: str) -> None:
    if needle not in text:
        raise RuntimeError(f"missing {label}: {needle}")


def main() -> int:
    platform = read("src-tauri/src/device_control_platform.rs")
    commands = read("src-tauri/src/commands.rs")
    ble = read("src-tauri/src/embedded_ble.rs")

    require(platform, "BleDeviceSettingsTransaction", "BLE settings transaction")
    require(platform, "OperationKind::ReadCapabilities", "capability operation")
    require(platform, "OperationKind::ReadSetting", "settings read operation")
    require(platform, "OperationKind::WriteSetting", "settings write operation")
    require(platform, "OperationKind::InvokeCommand", "command operation")
    require(platform, "device_control_v1", "firmware capability requirement")
    require(platform, "acknowledge_setting_write", "write acknowledgement")
    require(platform, "confirm_setting_readback", "persisted revision readback")
    require(platform, "format!(\"{command} rev={}\"", "guarded settings write")
    require(platform, "classify_ble_error_category", "classified terminal failures")
    require(commands, "BleDeviceSettingsTransaction::begin", "settings save integration")
    require(commands, "preserving fallback", "legacy USB/active-capture fallback")
    require(ble, "DEVICE_SETTINGS_REVISION_UUID_TEXT", "public settings revision UUID")
    require(ble, "read_device_settings_revision", "uncached BLE settings revision read")
    require(
        ble,
        "parse_device_settings_revision_characteristic",
        "settings revision parser",
    )
    print("PASS: DeviceControlCore BLE settings capability, revision, ACK/readback, timeout, and classified-result guards are present.")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except RuntimeError as error:
        print(f"FAIL: {error}", file=sys.stderr)
        raise SystemExit(1)
