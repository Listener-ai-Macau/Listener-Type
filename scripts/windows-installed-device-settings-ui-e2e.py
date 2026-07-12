#!/usr/bin/env python3
"""Exercise Listener Type device settings through the installed UI controls only.

The caller launches the Program Files app with a WebView remote-debugging port.
This script uses Chrome DevTools mouse/keyboard input to edit the visible form and
click its Read/Write buttons. It intentionally never calls Tauri settings commands.
"""

from __future__ import annotations

import argparse
import json
import secrets
import string
import sys
import time
import urllib.request
from pathlib import Path

from websockets.sync.client import connect


NUMBER_FIELD_NAMES = (
    "pluggedLowPowerIdleMinutes",
    "batteryLowPowerIdleMinutes",
    "batteryAutoShutdownMinutes",
    "statusLedBrightnessPercent",
    "keyLedBrightnessPercent",
    "knobLedBrightnessPercent",
    "edgeLedBrightnessPercent",
)


def cdp_page_ws(port: int) -> str:
    deadline = time.monotonic() + 20
    last_targets: list[str] = []
    while time.monotonic() < deadline:
        try:
            with urllib.request.urlopen(f"http://127.0.0.1:{port}/json/list", timeout=1) as response:
                targets = json.loads(response.read().decode("utf-8"))
            last_targets = [target.get("url", "") for target in targets]
            for target in targets:
                url = target.get("url", "")
                if url.startswith("http://tauri.localhost") and "?window=" not in url:
                    return target["webSocketDebuggerUrl"]
        except Exception:
            pass
        time.sleep(0.25)
    raise RuntimeError(f"main Listener Type page not found; targets={last_targets}")


class CdpClient:
    def __init__(self, websocket_url: str):
        self.ws = connect(websocket_url)
        self.next_id = 1

    def send(self, method: str, params: dict | None = None) -> dict:
        message_id = self.next_id
        self.next_id += 1
        payload = {"id": message_id, "method": method}
        if params is not None:
            payload["params"] = params
        self.ws.send(json.dumps(payload))
        while True:
            message = json.loads(self.ws.recv())
            if message.get("id") == message_id:
                if "error" in message:
                    raise RuntimeError(f"CDP {method} failed: {message['error']}")
                return message.get("result", {})

    def evaluate(self, expression: str):
        result = self.send(
            "Runtime.evaluate",
            {"expression": expression, "returnByValue": True, "awaitPromise": True},
        )
        if "exceptionDetails" in result:
            raise RuntimeError(json.dumps(result["exceptionDetails"], ensure_ascii=False))
        return result.get("result", {}).get("value")

    def click(self, point: dict[str, float]) -> None:
        for event_type in ("mousePressed", "mouseReleased"):
            self.send(
                "Input.dispatchMouseEvent",
                {
                    "type": event_type,
                    "x": point["x"],
                    "y": point["y"],
                    "button": "left",
                    "clickCount": 1,
                },
            )

    def key(self, event_type: str, key: str, code: str, virtual_key_code: int, modifiers: int = 0) -> None:
        self.send(
            "Input.dispatchKeyEvent",
            {
                "type": event_type,
                "key": key,
                "code": code,
                "windowsVirtualKeyCode": virtual_key_code,
                "modifiers": modifiers,
            },
        )

    def replace_focused_text(self, value: str) -> None:
        self.key("rawKeyDown", "Control", "ControlLeft", 17, modifiers=2)
        self.key("keyDown", "a", "KeyA", 65, modifiers=2)
        self.key("keyUp", "a", "KeyA", 65, modifiers=2)
        self.key("keyUp", "Control", "ControlLeft", 17)
        self.key("keyDown", "Backspace", "Backspace", 8)
        self.key("keyUp", "Backspace", "Backspace", 8)
        self.send("Input.insertText", {"text": value})

    def screenshot(self, output_path: Path) -> None:
        result = self.send("Page.captureScreenshot", {"format": "png"})
        output_path.parent.mkdir(parents=True, exist_ok=True)
        import base64

        output_path.write_bytes(base64.b64decode(result["data"]))

    def close(self) -> None:
        self.ws.close()


def wait_for(predicate, timeout_seconds: float, error: str):
    deadline = time.monotonic() + timeout_seconds
    last = None
    while time.monotonic() < deadline:
        last = predicate()
        if last:
            return last
        time.sleep(0.2)
    raise RuntimeError(f"{error}; last={last}")


def ui_snapshot(client: CdpClient) -> dict:
    return client.evaluate(
        """
        (() => {
          const card = document.querySelector('.ol-device-settings-card');
          if (!card) return { ready: false, body: document.body.innerText.slice(0, 2000) };
          const numberInputs = [...card.querySelectorAll('input[type="number"]')];
          const bleNameInput = card.querySelector('input:not([type="number"])');
          const buttonRows = [...card.querySelectorAll('button')].map((button, index) => ({
            index,
            text: button.innerText.trim(),
            disabled: button.disabled,
          }));
          return {
            ready: true,
            bleName: bleNameInput?.value ?? null,
            numberInputs: numberInputs.map((input, index) => ({
              index,
              value: input.value,
              min: input.min,
              max: input.max,
              disabled: input.disabled,
              context: (input.closest('.ol-device-timing-control, .ol-device-led-row')?.innerText || '').trim(),
            })),
            buttons: buttonRows,
            text: card.innerText,
          };
        })()
        """
    )


def visible_button_center(client: CdpClient, *, text: str | None = None, title: str | None = None, within_overlay: bool = False) -> dict[str, float] | None:
    scope = "overlay" if within_overlay else "document"
    point = client.evaluate(
        f"""
        (() => {{
          const overlays = [...document.querySelectorAll('div')].filter(candidate => {{
            const style = getComputedStyle(candidate);
            const rect = candidate.getBoundingClientRect();
            return style.position === 'absolute' && Number(style.zIndex) >= 70 && rect.width > 0 && rect.height > 0;
          }});
          const overlay = overlays.at(-1);
          const scope = {scope};
          if (!scope) return null;
          const button = [...scope.querySelectorAll('button')].find(candidate => {{
            const rect = candidate.getBoundingClientRect();
            return rect.width > 0 && rect.height > 0
              && !candidate.disabled
              && ({json.dumps(text)} === null || candidate.innerText.trim() === {json.dumps(text)})
              && ({json.dumps(title)} === null || candidate.title === {json.dumps(title)});
          }});
          if (!button) return null;
          const rect = button.getBoundingClientRect();
          return {{ x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 }};
        }})()
        """
    )
    return point or None


def has_provider_setup_overlay(client: CdpClient) -> bool:
    return bool(client.evaluate(
        """
        (() => [...document.querySelectorAll('div')].some(candidate => {
          const style = getComputedStyle(candidate);
          const rect = candidate.getBoundingClientRect();
          return style.position === 'absolute' && Number(style.zIndex) >= 70
            && rect.width > 0 && rect.height > 0 && candidate.innerText.includes('稍后');
        }))()
        """
    ))


def ensure_device_settings_card(client: CdpClient) -> dict:
    snapshot = ui_snapshot(client)
    if snapshot.get("ready"):
        return snapshot

    if has_provider_setup_overlay(client):
        later = visible_button_center(client, text="稍后", within_overlay=True)
        if not later:
            raise RuntimeError("provider setup overlay is blocking settings but its Later button is unavailable")
        client.click(later)
        wait_for(lambda: not has_provider_setup_overlay(client), 5, "provider setup overlay did not dismiss")

    settings_button = visible_button_center(client, title="设置")
    if not settings_button:
        raise RuntimeError("main-window Settings button is unavailable")
    client.click(settings_button)

    def open_device_section() -> dict | None:
        card = ui_snapshot(client)
        if card.get("ready"):
            return card
        device_button = visible_button_center(client, text="设备")
        if device_button:
            client.click(device_button)
        card = ui_snapshot(client)
        return card if card.get("ready") else None

    return wait_for(open_device_section, 20, "device settings card did not render after normal UI navigation")


def input_center(client: CdpClient, index: int) -> dict[str, float]:
    point = client.evaluate(
        f"""
        (() => {{
          const card = document.querySelector('.ol-device-settings-card');
          const input = card?.querySelectorAll('input[type="number"]')[{index}];
          if (!input) return null;
          input.scrollIntoView({{ block: 'center', inline: 'nearest' }});
          const rect = input.getBoundingClientRect();
          return {{ x: rect.left + rect.width / 2, y: rect.top + rect.height / 2, visible: rect.width > 0 && rect.height > 0 }};
        }})()
        """
    )
    if not point or not point.get("visible"):
        raise RuntimeError(f"device-settings number input {index} is not visible")
    return point


def enabled_ble_name_input_center(client: CdpClient) -> dict[str, float] | None:
    return client.evaluate(
        """
        (() => {
          const card = document.querySelector('.ol-device-settings-card');
          const input = card?.querySelector('input:not([type="number"])');
          if (!input || input.disabled) return null;
          input.scrollIntoView({ block: 'center', inline: 'nearest' });
          const rect = input.getBoundingClientRect();
          return { x: rect.left + rect.width / 2, y: rect.top + rect.height / 2, visible: rect.width > 0 && rect.height > 0 };
        })()
        """
    )
def ble_name_input_center(client: CdpClient) -> dict[str, float]:
    point = wait_for(
        lambda: enabled_ble_name_input_center(client),
        3,
        "visible enabled BLE-name input not found",
    )
    if not point or not point.get("visible"):
        raise RuntimeError("visible BLE-name input not found")
    return point


def card_button_center(client: CdpClient, text: str) -> dict[str, float] | None:
    return client.evaluate(
        f"""
        (() => {{
          const button = [...document.querySelectorAll('.ol-device-settings-card button')]
            .find(candidate => candidate.innerText.trim() === {json.dumps(text)});
          if (!button || button.disabled) return null;
          button.scrollIntoView({{ block: 'center', inline: 'nearest' }});
          const rect = button.getBoundingClientRect();
          return {{ x: rect.left + rect.width / 2, y: rect.top + rect.height / 2, visible: rect.width > 0 && rect.height > 0 }};
        }})()
        """
    )


def button_center(client: CdpClient, text: str) -> dict[str, float]:
    point = wait_for(
        lambda: card_button_center(client, text),
        3,
        f"visible enabled device-settings button not found: {text}",
    )
    if not point or not point.get("visible"):
        raise RuntimeError(f"visible enabled device-settings button not found: {text}")
    return point


def read_device_settings(client: CdpClient, max_wait_ms: int) -> dict:
    client.click(button_center(client, "读取"))
    return wait_for(
        lambda: (
            (snapshot := ui_snapshot(client)).get("ready")
            and all(button["text"] != "读取中" for button in snapshot.get("buttons", []))
            and snapshot
        ),
        max(max_wait_ms / 1000, 2.5),
        "device UI did not finish the settings refresh",
    )


def wait_for_device_settings_write_to_settle(client: CdpClient, max_wait_ms: int) -> dict:
    return wait_for(
        lambda: (
            (snapshot := ui_snapshot(client)).get("ready")
            and all(button["text"] != "写入中" for button in snapshot.get("buttons", []))
            and snapshot
        ),
        max(max_wait_ms / 1000, 20),
        "device UI write did not settle before restore",
    )


def numeric_form_values(snapshot: dict) -> dict[str, int]:
    inputs = snapshot.get("numberInputs", [])
    if len(inputs) != len(NUMBER_FIELD_NAMES):
        raise RuntimeError(f"expected {len(NUMBER_FIELD_NAMES)} device-setting number inputs, found {len(inputs)}: {inputs}")
    return {name: int(inputs[index]["value"]) for index, name in enumerate(NUMBER_FIELD_NAMES)}


def different_random(current: int, low: int, high: int) -> int:
    value = secrets.randbelow(high - low + 1) + low
    return value if value != current else low + ((value - low + 1) % (high - low + 1))


def random_test_values(before: dict[str, int]) -> dict[str, int]:
    return {
        "pluggedLowPowerIdleMinutes": different_random(before["pluggedLowPowerIdleMinutes"], 1, 2),
        "batteryLowPowerIdleMinutes": different_random(before["batteryLowPowerIdleMinutes"], 3, 29),
        "batteryAutoShutdownMinutes": before["batteryAutoShutdownMinutes"],
        "statusLedBrightnessPercent": different_random(before["statusLedBrightnessPercent"], 31, 87),
        "keyLedBrightnessPercent": different_random(before["keyLedBrightnessPercent"], 33, 89),
        "knobLedBrightnessPercent": different_random(before["knobLedBrightnessPercent"], 35, 91),
        "edgeLedBrightnessPercent": different_random(before["edgeLedBrightnessPercent"], 37, 93),
    }


def apply_and_read_back(client: CdpClient, expected: dict[str, int], max_write_ms: int) -> tuple[dict, dict]:
    before = ui_snapshot(client)
    if not before.get("ready"):
        raise RuntimeError(f"device settings UI not visible: {before}")
    numeric_form_values(before)

    for index, name in enumerate(NUMBER_FIELD_NAMES):
        point = input_center(client, index)
        client.click(point)
        client.replace_focused_text(str(expected[name]))

    typed = numeric_form_values(ui_snapshot(client))
    if typed != expected:
        raise RuntimeError(f"UI input values differ before Write: expected={expected} actual={typed}")

    write_started = time.monotonic()
    client.click(button_center(client, "写入"))
    wait_for(
        lambda: "已发送到设备" in str(ui_snapshot(client).get("text", "")),
        max_write_ms / 1000,
        "device UI did not show saved confirmation",
    )
    write_elapsed_ms = round((time.monotonic() - write_started) * 1000)

    client.click(button_center(client, "读取"))
    read_back = wait_for(
        lambda: (
            (snapshot := ui_snapshot(client)).get("ready")
            and numeric_form_values(snapshot) == expected
            and snapshot
        ),
        max_write_ms / 1000,
        "device UI Read did not return the written values",
    )
    return read_back, {"writeElapsedMs": write_elapsed_ms, "typed": typed}


def write_ble_name_and_read_back(
    client: CdpClient,
    ble_name: str,
    max_write_ms: int,
) -> tuple[dict, dict]:
    before = ui_snapshot(client)
    if not before.get("ready"):
        raise RuntimeError(f"device settings UI not visible: {before}")
    before_numeric = numeric_form_values(before)

    client.click(ble_name_input_center(client))
    client.replace_focused_text(ble_name)
    typed = ui_snapshot(client)
    numeric_form_values(typed)
    if typed.get("bleName") != ble_name:
        raise RuntimeError(
            f"BLE-name UI input did not retain the requested name: expected_name={ble_name!r} actual={typed.get('bleName')!r}"
        )

    write_started = time.monotonic()
    client.click(button_center(client, "写入"))
    wait_for(
        lambda: "已发送到设备" in str(ui_snapshot(client).get("text", "")),
        max_write_ms / 1000,
        "BLE-name device UI write did not show saved confirmation",
    )
    write_elapsed_ms = round((time.monotonic() - write_started) * 1000)

    client.click(button_center(client, "读取"))
    read_back = wait_for(
        lambda: (
            (snapshot := ui_snapshot(client)).get("ready")
            and snapshot.get("bleName") == ble_name
            and numeric_form_values(snapshot) == before_numeric
            and snapshot
        ),
        max_write_ms / 1000,
        "BLE-name device UI Read did not return the requested name and unchanged numeric values",
    )
    return read_back, {"writeElapsedMs": write_elapsed_ms, "bleName": ble_name, "numericValues": before_numeric}


def random_ble_name(excluding: str) -> str:
    alphabet = string.ascii_letters + string.digits
    while True:
        candidate = "".join(secrets.choice(alphabet) for _ in range(12))
        if candidate != excluding and all(token not in candidate.lower() for token in ("listener", "type", "lt")):
            return candidate


def same_name_log_evidence(log_path: Path, start_offset: int, output_json: Path) -> dict:
    if not log_path.exists():
        raise RuntimeError(f"Type log does not exist: {log_path}")
    output_json.parent.mkdir(parents=True, exist_ok=True)
    with log_path.open("rb") as stream:
        stream.seek(start_offset)
        delta = stream.read().decode("utf-8", errors="replace")
    delta_path = output_json.with_suffix(".same-name-type-log-delta.log")
    delta_path.write_text(delta, encoding="utf-8")
    forbidden_tokens = (
        "DEVICE:SET ble_name=",
        "apply pending BLE name",
        "PairAsync",
        "Windows cache refresh",
        "Windows pairing prompt",
    )
    forbidden = {
        token: [line for line in delta.splitlines() if token.lower() in line.lower()]
        for token in forbidden_tokens
    }
    forbidden = {token: lines for token, lines in forbidden.items() if lines}
    return {
        "path": str(log_path),
        "startByteOffset": start_offset,
        "deltaPath": str(delta_path),
        "writePlanBleNameUnchanged": "ble_name_changed=false apply_needed=false" in delta,
        "forbiddenMatches": forbidden,
    }


def changed_name_log_evidence(
    log_path: Path,
    start_offset: int,
    expected_name: str,
    output_json: Path,
    phase: str,
) -> dict:
    if not log_path.exists():
        raise RuntimeError(f"Type log does not exist: {log_path}")
    output_json.parent.mkdir(parents=True, exist_ok=True)
    with log_path.open("rb") as stream:
        stream.seek(start_offset)
        delta = stream.read().decode("utf-8", errors="replace")
    delta_path = output_json.with_name(f"{output_json.stem}.{phase}-type-log-delta.log")
    delta_path.write_text(delta, encoding="utf-8")
    lines = delta.splitlines()
    pair_indices = [
        index
        for index, line in enumerate(lines)
        if "device BLE name change Windows PairAsync" in line
    ]
    pair_lines = [lines[index] for index in pair_indices]
    notify_ready_lines = [
        line
        for line in lines[(pair_indices[-1] + 1) if pair_indices else len(lines):]
        if "background listener notify ready" in line
    ]
    required = {
        "firmware_name_write": f"command=DEVICE:SET ble_name={expected_name}" in delta,
        "name_apply": "ble_name_changed=true apply_needed=true" in delta,
        "cache_refresh": "BLE name Windows cache refresh" in delta,
        "silent_pairing_policy": any("allow_user_prompt=false" in line for line in pair_lines),
        "no_bluetooth_settings": any("open_settings=false" in line for line in pair_lines),
        "pairing_completed": any(
            "status=Paired" in line or "status=AlreadyPaired" in line for line in pair_lines
        ),
        "notify_ready": bool(notify_ready_lines),
    }
    return {
        "path": str(log_path),
        "startByteOffset": start_offset,
        "deltaPath": str(delta_path),
        "pairLines": pair_lines,
        "notifyReadyLines": notify_ready_lines,
        "required": required,
    }


def wait_for_listener_notify_ready(
    log_path: Path,
    start_offset: int,
    started_at: float,
    max_total_ms: int,
    phase: str,
) -> dict:
    deadline = started_at + max_total_ms / 1000
    last_delta = ""
    while time.monotonic() < deadline:
        with log_path.open("rb") as stream:
            stream.seek(start_offset)
            last_delta = stream.read().decode("utf-8", errors="replace")
        lines = last_delta.splitlines()
        pair_indices = [
            index
            for index, line in enumerate(lines)
            if "device BLE name change Windows PairAsync" in line
        ]
        ready_lines = [
            line
            for line in lines[(pair_indices[-1] + 1) if pair_indices else len(lines):]
            if "background listener notify ready" in line
        ]
        if ready_lines:
            return {
                "elapsedMs": round((time.monotonic() - started_at) * 1000),
                "line": ready_lines[-1],
            }
        time.sleep(0.05)
    raise RuntimeError(
        f"{phase} did not restore Listener notify subscription within {max_total_ms} ms; "
        f"last_log_tail={last_delta[-600:]}"
    )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--remote-debugging-port", type=int, required=True)
    parser.add_argument("--output-json", type=Path, required=True)
    parser.add_argument("--restore-from-json", type=Path)
    parser.add_argument("--read-only", action="store_true")
    parser.add_argument("--same-name-write", action="store_true")
    parser.add_argument("--set-ble-name")
    parser.add_argument("--different-random-name-roundtrip", action="store_true")
    parser.add_argument("--type-log", type=Path)
    parser.add_argument("--max-write-ms", type=int, default=2500)
    parser.add_argument("--max-rename-total-ms", type=int, default=10000)
    args = parser.parse_args()

    client = CdpClient(cdp_page_ws(args.remote_debugging_port))
    try:
        ensure_device_settings_card(client)
        start_snapshot = read_device_settings(client, args.max_write_ms)
        before = numeric_form_values(start_snapshot)
        if args.same_name_write and (args.read_only or args.restore_from_json or args.set_ble_name or args.different_random_name_roundtrip):
            raise RuntimeError("--same-name-write cannot be combined with another settings operation")
        if args.set_ble_name and (args.read_only or args.restore_from_json or args.different_random_name_roundtrip):
            raise RuntimeError("--set-ble-name cannot be combined with another settings operation")
        if args.different_random_name_roundtrip and (args.read_only or args.restore_from_json or args.set_ble_name):
            raise RuntimeError("--different-random-name-roundtrip cannot be combined with another settings operation")
        if args.read_only:
            current = numeric_form_values(start_snapshot)
            result = {
                "schema": "listener.installed_type_device_settings_ui_e2e.v1",
                "ok": True,
                "phase": "read_only",
                "displayed_after_read": current,
                "interaction": "Chrome DevTools Input mouse click against visible Program Files Type UI; no Tauri settings invoke",
            }
            args.output_json.parent.mkdir(parents=True, exist_ok=True)
            args.output_json.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")
            print(json.dumps(result, ensure_ascii=False, indent=2))
            return 0
        if args.set_ble_name:
            if not args.type_log:
                raise RuntimeError("--set-ble-name requires --type-log for recovery-path evidence")
            target_name = args.set_ble_name.strip()
            if not target_name:
                raise RuntimeError("--set-ble-name requires a non-empty name")
            log_start_offset = args.type_log.stat().st_size
            started_at = time.monotonic()
            read_back, timing = write_ble_name_and_read_back(client, target_name, args.max_write_ms)
            notify_ready = wait_for_listener_notify_ready(
                args.type_log,
                log_start_offset,
                started_at,
                args.max_rename_total_ms,
                "requested BLE-name write",
            )
            log_evidence = changed_name_log_evidence(
                args.type_log,
                log_start_offset,
                target_name,
                args.output_json,
                "requested-name",
            )
            result = {
                "schema": "listener.installed_type_device_settings_ui_e2e.v1",
                "ok": read_back.get("bleName") == target_name and all(log_evidence["required"].values()),
                "phase": "set_ble_name",
                "requestedName": target_name,
                "displayedAfterRead": read_back.get("bleName"),
                "timing": {
                    "writeElapsedMs": timing["writeElapsedMs"],
                    "notifyReadyElapsedMs": notify_ready["elapsedMs"],
                },
                "typeLog": log_evidence,
                "interaction": "Chrome DevTools Input mouse/keyboard events against visible Program Files Type UI; no Tauri settings invoke",
            }
            args.output_json.parent.mkdir(parents=True, exist_ok=True)
            args.output_json.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")
            print(json.dumps(result, ensure_ascii=False, indent=2))
            return 0 if result["ok"] else 2
        if args.same_name_write:
            if not args.type_log:
                raise RuntimeError("--same-name-write requires --type-log for recovery-path evidence")
            if not args.type_log.exists():
                raise RuntimeError(f"Type log does not exist: {args.type_log}")
            log_start_offset = args.type_log.stat().st_size
            current_name = start_snapshot.get("bleName")
            if not isinstance(current_name, str) or not current_name:
                raise RuntimeError(f"visible BLE name is unavailable: {current_name!r}")
            read_back, timing = write_ble_name_and_read_back(client, current_name, args.max_write_ms)
            time.sleep(0.4)
            log_evidence = same_name_log_evidence(args.type_log, log_start_offset, args.output_json)
            result = {
                "schema": "listener.installed_type_device_settings_ui_e2e.v1",
                "ok": not log_evidence["forbiddenMatches"] and log_evidence["writePlanBleNameUnchanged"],
                "phase": "same_name_write",
                "before": {"bleName": timing["bleName"], **timing["numericValues"]},
                "displayed_after_read": {"bleName": read_back["bleName"], **numeric_form_values(read_back)},
                "timing": {"writeElapsedMs": timing["writeElapsedMs"]},
                "typeLog": log_evidence,
                "interaction": "Chrome DevTools Input mouse/keyboard events against visible Program Files Type UI; no Tauri settings invoke",
            }
            args.output_json.parent.mkdir(parents=True, exist_ok=True)
            args.output_json.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")
            print(json.dumps(result, ensure_ascii=False, indent=2))
            return 0 if result["ok"] else 2
        if args.different_random_name_roundtrip:
            if not args.type_log:
                raise RuntimeError("--different-random-name-roundtrip requires --type-log for recovery-path evidence")
            if not args.type_log.exists():
                raise RuntimeError(f"Type log does not exist: {args.type_log}")
            args.output_json.parent.mkdir(parents=True, exist_ok=True)
            original_name = start_snapshot.get("bleName")
            if not isinstance(original_name, str) or not original_name:
                raise RuntimeError(f"visible BLE name is unavailable: {original_name!r}")
            target_name = random_ble_name(original_name)
            random_log_offset = args.type_log.stat().st_size
            random_readback = None
            random_timing = None
            random_log = None
            random_notify_ready = None
            random_error = None
            try:
                random_started_at = time.monotonic()
                random_readback, random_timing = write_ble_name_and_read_back(client, target_name, args.max_write_ms)
                random_notify_ready = wait_for_listener_notify_ready(
                    args.type_log,
                    random_log_offset,
                    random_started_at,
                    args.max_rename_total_ms,
                    "random BLE-name write",
                )
                random_log = changed_name_log_evidence(
                    args.type_log,
                    random_log_offset,
                    target_name,
                    args.output_json,
                    "random-name",
                )
            except Exception as error:
                random_error = str(error)
                try:
                    wait_for_device_settings_write_to_settle(client, args.max_write_ms)
                    random_readback = read_device_settings(client, args.max_write_ms)
                    time.sleep(0.4)
                    random_log = changed_name_log_evidence(
                        args.type_log,
                        random_log_offset,
                        target_name,
                        args.output_json,
                        "random-name",
                    )
                except Exception as settle_error:
                    random_error = f"{random_error}; recovery before restore failed: {settle_error}"

            restore_readback = None
            restore_timing = None
            restore_log = None
            restore_notify_ready = None
            restore_error = None
            try:
                restore_log_offset = args.type_log.stat().st_size
                restore_started_at = time.monotonic()
                restore_readback, restore_timing = write_ble_name_and_read_back(client, original_name, args.max_write_ms)
                restore_notify_ready = wait_for_listener_notify_ready(
                    args.type_log,
                    restore_log_offset,
                    restore_started_at,
                    args.max_rename_total_ms,
                    "restore BLE-name write",
                )
                restore_log = changed_name_log_evidence(
                    args.type_log,
                    restore_log_offset,
                    original_name,
                    args.output_json,
                    "restore-name",
                )
            except Exception as error:
                restore_error = str(error)
            result = {
                "schema": "listener.installed_type_device_settings_ui_e2e.v1",
                "ok": (
                    random_error is None
                    and restore_error is None
                    and random_readback is not None
                    and restore_readback is not None
                    and random_log is not None
                    and restore_log is not None
                    and random_notify_ready is not None
                    and restore_notify_ready is not None
                    and random_readback.get("bleName") == target_name
                    and restore_readback.get("bleName") == original_name
                    and all(random_log["required"].values())
                    and all(restore_log["required"].values())
                ),
                "phase": "different_random_name_roundtrip",
                "originalName": original_name,
                "randomName": target_name,
                "randomReadback": random_readback.get("bleName") if random_readback else None,
                "restoreReadback": restore_readback.get("bleName") if restore_readback else None,
                "timing": {
                    "randomWriteElapsedMs": random_timing["writeElapsedMs"] if random_timing else None,
                    "restoreWriteElapsedMs": restore_timing["writeElapsedMs"] if restore_timing else None,
                    "randomNotifyReadyElapsedMs": random_notify_ready["elapsedMs"] if random_notify_ready else None,
                    "restoreNotifyReadyElapsedMs": restore_notify_ready["elapsedMs"] if restore_notify_ready else None,
                },
                "randomNameLog": random_log,
                "restoreNameLog": restore_log,
                "randomError": random_error,
                "restoreError": restore_error,
                "interaction": "Chrome DevTools Input mouse/keyboard events against visible Program Files Type UI; no Tauri settings invoke",
            }
            args.output_json.parent.mkdir(parents=True, exist_ok=True)
            args.output_json.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")
            print(json.dumps(result, ensure_ascii=False, indent=2))
            return 0 if result["ok"] else 2
        if args.restore_from_json:
            source = json.loads(args.restore_from_json.read_text(encoding="utf-8"))
            expected = {name: int(source["before"][name]) for name in NUMBER_FIELD_NAMES}
            phase = "restore"
        else:
            expected = random_test_values(before)
            phase = "random_write"

        args.output_json.parent.mkdir(parents=True, exist_ok=True)
        client.screenshot(args.output_json.with_suffix(".before.png"))
        read_back, timing = apply_and_read_back(client, expected, args.max_write_ms)
        client.screenshot(args.output_json.with_suffix(".after.png"))
        result = {
            "schema": "listener.installed_type_device_settings_ui_e2e.v1",
            "ok": True,
            "phase": phase,
            "before": before,
            "requested": expected,
            "displayed_after_read": numeric_form_values(read_back),
            "timing": timing,
            "interaction": "Chrome DevTools Input mouse/keyboard events against visible Program Files Type UI; no Tauri settings invoke",
        }
        args.output_json.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")
        print(json.dumps(result, ensure_ascii=False, indent=2))
        return 0
    except Exception as exc:
        result = {"schema": "listener.installed_type_device_settings_ui_e2e.v1", "ok": False, "error": str(exc)}
        args.output_json.parent.mkdir(parents=True, exist_ok=True)
        args.output_json.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")
        print(json.dumps(result, ensure_ascii=False, indent=2), file=sys.stderr)
        return 1
    finally:
        client.close()


if __name__ == "__main__":
    raise SystemExit(main())
