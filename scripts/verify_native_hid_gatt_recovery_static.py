"""Static guard for Windows native-HID GATT recovery after service-table changes."""

from __future__ import annotations

import pathlib
import sys


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
SOURCE_PATH = REPO_ROOT / "src-tauri" / "src" / "embedded_ble.rs"


def require(source: str, fragment: str, message: str) -> None:
    if fragment not in source:
        raise AssertionError(message)


def main() -> int:
    source = SOURCE_PATH.read_text(encoding="utf-8")

    require(
        source,
        "BluetoothLEDevice::FromBluetoothAddressWithBluetoothAddressTypeAsync(",
        "native HID random identity must use the typed Bluetooth address open",
    )
    require(
        source,
        "BluetoothAddressType::Random",
        "native HID random identity must be explicit",
    )
    require(
        source,
        "fn open_notify_target_for_current_native_windows_hid_service_endpoint(",
        "native HID needs an exact service-id recovery endpoint",
    )
    require(
        source,
        "addresses.contains(&address)",
        "native HID service endpoint must be restricted to the current address",
    )
    require(
        source,
        "open_notify_target_for_service_with_timeout(&id, timeout)",
        "native HID service endpoint must use the bounded service-id open",
    )
    require(
        source,
        "require_audio_control_for_notify_target(",
        "native HID notify recovery must not enter capture without audio control",
    )
    require(
        source,
        "BLE audio control unavailable while opening notify target",
        "missing audio control must remain a classified retryable notify-open failure",
    )
    require(
        source,
        "BluetoothCacheMode::Uncached",
        "native HID service endpoint must keep uncached GATT reads",
    )
    require(
        source,
        "current native Windows HID service-id endpoint after direct GATT miss",
        "native HID direct GATT miss must reach the exact service-id endpoint",
    )
    require(
        source,
        "fn open_listener_ota_v1_target_for_current_native_windows_hid_service_endpoint(",
        "OTA needs the same exact native-HID service-id recovery endpoint",
    )
    require(
        source,
        "open_listener_ota_v1_target_for_service_with_cache_policy(&id, true)",
        "native-HID OTA recovery needs the exact service-id compatibility fallback",
    )
    require(
        source,
        "if allow_cached {\n            &[BluetoothCacheMode::Uncached, BluetoothCacheMode::Cached]",
        "OTA service-id compatibility must still try uncached GATT first",
    )
    require(
        source,
        'lower.contains("0x80070016")',
        "Windows ERROR_BAD_COMMAND must classify as stale GATT",
    )
    require(
        source,
        'err.contains("HRESULT(0x80070016)")',
        "Windows ERROR_BAD_COMMAND must be retried as a GATT transient",
    )

    print("PASS: native Windows HID GATT recovery keeps random identity, exact audio/OTA service-id fallback, uncached reads, and stale-GATT classification.")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except AssertionError as error:
        print(f"FAIL: {error}", file=sys.stderr)
        raise SystemExit(1)
