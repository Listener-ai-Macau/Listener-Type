from __future__ import annotations

import sys
from pathlib import Path


TYPE_REPO_ROOT = Path(__file__).resolve().parents[1]
FIRMWARE_REPO_ROOT = TYPE_REPO_ROOT.parent / "Listener-Firmware"


def read_source(path: Path, failures: list[str]) -> str:
    if not path.is_file():
        failures.append(f"missing required source: {path}")
        return ""
    return path.read_text(encoding="utf-8")


def require_fragment(source: str, label: str, fragment: str, failures: list[str]) -> None:
    if fragment not in source:
        failures.append(f"{label}: missing {fragment!r}")


def require_ordered(
    source: str,
    label: str,
    earlier: str,
    later: str,
    failures: list[str],
) -> None:
    earlier_index = source.find(earlier)
    later_index = source.find(later)
    if earlier_index < 0 or later_index < 0:
        failures.append(f"{label}: missing ordered fragments {earlier!r} or {later!r}")
    elif earlier_index >= later_index:
        failures.append(f"{label}: expected {earlier!r} before {later!r}")


def function_slice(source: str, start: str, end: str, label: str, failures: list[str]) -> str:
    start_index = source.find(start)
    if start_index < 0:
        failures.append(f"{label}: missing function start {start!r}")
        return ""
    end_index = source.find(end, start_index + len(start))
    if end_index < 0:
        failures.append(f"{label}: missing function end {end!r}")
        return ""
    return source[start_index:end_index]


def main() -> int:
    failures: list[str] = []
    type_ble_path = TYPE_REPO_ROOT / "src-tauri/src/embedded_ble.rs"
    coordinator_path = TYPE_REPO_ROOT / "src-tauri/src/coordinator.rs"
    firmware_gap_path = FIRMWARE_REPO_ROOT / "ports/esp32/ble_hid_gap/ble_hid_gap_esp32.c"

    type_ble = read_source(type_ble_path, failures)
    coordinator = read_source(coordinator_path, failures)
    firmware_gap = read_source(firmware_gap_path, failures)

    # An observed recovery address is authoritative. A direct PairAsync failure
    # must not expand into an all-device AEP/PnP scan that hides a fast failure.
    for fragment in (
        "let exact_recovery_address = !observed_recovery_addresses.is_empty();",
        "fresh recovery-address direct PairAsync failed; skipping slow AEP discovery",
        "fn clear_listener_bthport_cache_for_known_addresses_inner",
        "exact_address_only: bool",
        "|| (!exact_address_only && (name_matches || entry.has_listener_service_signature))",
    ):
        require_fragment(type_ble, "Type embedded BLE", fragment, failures)
    require_ordered(
        type_ble,
        "Type embedded BLE",
        "if exact_recovery_address {",
        "listener_recovery_pairing_selector_fallback_candidates_for_addresses(",
        failures,
    )

    exact_cache = function_slice(
        type_ble,
        "fn clear_listener_bthport_cache_for_known_addresses_inner",
        "fn finalize_known_address_unpair_result",
        "Type exact cache cleanup",
        failures,
    )
    require_fragment(
        exact_cache,
        "Type exact cache cleanup",
        "bthport_listener_cache_candidates(&target_addresses, &target_names, true)",
        failures,
    )
    if "listener_pnp_remove_candidates" in exact_cache:
        failures.append(
            "Type exact cache cleanup: must not enumerate all Windows PnP devices"
        )

    coordinator_recovery = function_slice(
        coordinator,
        "async fn maybe_attempt_embedded_ble_background_stale_pairing_cleanup",
        "#[cfg(test)]",
        "Type recovery coordinator",
        failures,
    )
    require_ordered(
        coordinator_recovery,
        "Type recovery coordinator",
        "unpair_listener_pairing_for_known_addresses",
        "prompt_listener_pairing_after_type_recovery_without_user_prompt_after_cache_cleanup_for_addresses",
        failures,
    )
    require_ordered(
        coordinator_recovery,
        "Type recovery coordinator",
        "prompt_listener_pairing_after_type_recovery_without_user_prompt_after_cache_cleanup_for_addresses",
        "clear_listener_bthport_cache_for_known_addresses",
        failures,
    )
    fallback_start = coordinator_recovery.find(
        "fresh-address direct PairAsync did not complete after pairing-only cleanup"
    )
    if fallback_start < 0:
        failures.append("Type recovery coordinator: missing direct PairAsync failure fallback")
    elif "unpair_listener_devices_for_known_addresses" in coordinator_recovery[fallback_start:]:
        failures.append(
            "Type recovery coordinator: direct PairAsync failure must not fall through to full PnP/cache cleanup"
        )

    # The Device Control transaction remains open through retryable controller
    # failures and completes only when Type proves the connection is usable.
    mark_ready = function_slice(
        firmware_gap,
        "static void ble_hid_gap_platform_device_control_mark_type_ready(void)",
        "static void ble_hid_gap_platform_device_control_begin_recovery",
        "Firmware TYPE:READY completion",
        failures,
    )
    require_ordered(
        mark_ready,
        "Firmware TYPE:READY completion",
        "ble_hid_gap_platform_device_control_complete_recovery(",
        "denzic_device_control_v1_set_ownership(",
        failures,
    )
    for fragment in (
        "request.timeout_ms = 120000u;",
        "static void ble_hid_gap_platform_device_control_note_retryable_security_failure(int status)",
        "status == BLE_ERR_MEM_CAPACITY",
        "DENZIC_DEVICE_CONTROL_V1_ERROR_CATEGORY_RESOURCE",
        "device-control recovery security attempt retryable",
        "ble_hid_gap_platform_device_control_note_retryable_security_failure(\n                    event->enc_change.status);",
        "DENZIC_DEVICE_CONTROL_V1_OPERATION_RESULT_TIMED_OUT",
    ):
        require_fragment(firmware_gap, "Firmware device control", fragment, failures)

    warmup_advertising_acceptance = function_slice(
        firmware_gap,
        "static esp_err_t ble_hid_gap_start_type_recovery_warmup_advertising(void)",
        "esp_err_t esp_hid_ble_gap_adv_start(void)",
        "Firmware warm-up advertising acceptance",
        failures,
    )
    if "ble_hid_gap_platform_device_control_complete_recovery" in warmup_advertising_acceptance:
        failures.append(
            "Firmware warm-up advertising acceptance: starting advertising must not complete recovery"
        )

    advertising_acceptance = function_slice(
        firmware_gap,
        "/* A zero return is NimBLE's successful advertising-start acceptance.",
        "s_last_adv_was_directed = false;",
        "Firmware advertising acceptance",
        failures,
    )
    if "ble_hid_gap_platform_device_control_complete_recovery" in advertising_acceptance:
        failures.append(
            "Firmware advertising acceptance: starting advertising must not complete recovery"
        )

    if failures:
        print("FAIL: device-control recovery root-cause contract")
        print("\n".join(failures))
        return 1

    print("PASS: device-control recovery root-cause contract")
    return 0


if __name__ == "__main__":
    sys.exit(main())
