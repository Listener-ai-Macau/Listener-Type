import argparse
import json
import secrets
import string
import sys
import time
import urllib.request
from pathlib import Path

from websockets.sync.client import connect


def cdp_page_ws(port: int) -> str:
    deadline = time.time() + 20
    last_targets: list[str] = []
    while time.time() < deadline:
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
        time.sleep(0.3)
    raise RuntimeError(f"Main Tauri page target was not found. last_targets={last_targets}")


class CdpClient:
    def __init__(self, websocket_url: str):
        self.ws = connect(websocket_url)
        self.next_id = 1
        self._send("Runtime.enable")

    def _send(self, method: str, params: dict | None = None) -> dict:
        msg_id = self.next_id
        self.next_id += 1
        payload = {"id": msg_id, "method": method}
        if params is not None:
            payload["params"] = params
        self.ws.send(json.dumps(payload))
        while True:
            message = json.loads(self.ws.recv())
            if message.get("id") == msg_id:
                return message

    def evaluate(self, expression: str):
        response = self._send(
            "Runtime.evaluate",
            {
                "expression": expression,
                "returnByValue": True,
                "awaitPromise": True,
            },
        )
        if "exceptionDetails" in response.get("result", {}):
            raise RuntimeError(json.dumps(response["result"]["exceptionDetails"], ensure_ascii=False))
        return response["result"]["result"].get("value")

    def invoke(self, command: str, args: dict | None = None):
        args_json = json.dumps(args or {}, ensure_ascii=False)
        expression = f"""
        (async () => {{
          const value = await window.__TAURI__.core.invoke({json.dumps(command)}, {args_json});
          return JSON.stringify(value ?? null);
        }})()
        """
        raw = self.evaluate(expression)
        return json.loads(raw) if raw else None

    def close(self):
        self.ws.close()


def invoke_retry(client: CdpClient, command: str, args: dict | None = None, attempts: int = 5):
    last_error: Exception | None = None
    for _ in range(attempts):
        try:
            return client.invoke(command, args)
        except Exception as exc:
            last_error = exc
            time.sleep(1)
    raise RuntimeError(f"Tauri command failed after {attempts} attempts: {command}: {last_error}")


def request_from_snapshot(snapshot: dict) -> dict:
    return {
        "statusLedBrightnessPercent": int(snapshot["statusLedBrightnessPercent"]),
        "keyLedBrightnessPercent": int(snapshot["keyLedBrightnessPercent"]),
        "knobLedBrightnessPercent": int(snapshot["knobLedBrightnessPercent"]),
        "edgeLedBrightnessPercent": int(snapshot["edgeLedBrightnessPercent"]),
        "pluggedLowPowerIdleMinutes": int(snapshot["pluggedLowPowerIdleMinutes"]),
        "batteryLowPowerIdleMinutes": int(snapshot["batteryLowPowerIdleMinutes"]),
        "pluggedLowPowerEnabled": bool(snapshot["pluggedLowPowerEnabled"]),
        "pluggedAutoShutdownMinutes": int(snapshot.get("pluggedAutoShutdownMs", 0)) // 60000,
        "batteryAutoShutdownMinutes": int(snapshot.get("batteryAutoShutdownMs", 0)) // 60000,
        "bleName": snapshot["bleName"],
    }


def different(seed: int, current: int, low: int, high: int) -> int:
    value = low + (seed % (high - low + 1))
    if value == current:
        value = low + ((seed + 7) % (high - low + 1))
    return value


def random_ble_name(current: str) -> str:
    alphabet = string.ascii_letters + string.digits
    for _ in range(20):
        value = "".join(secrets.choice(alphabet) for _ in range(12))
        if value != current:
            return value
    raise RuntimeError("failed to generate a different random BLE name")


def compare_fields(actual: dict, expected: dict, fields: list[str]) -> list[dict]:
    mismatches = []
    for field in fields:
        if actual.get(field) != expected.get(field):
            mismatches.append(
                {
                    "field": field,
                    "expected": expected.get(field),
                    "actual": actual.get(field),
                }
            )
    return mismatches


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--remote-debugging-port", type=int, required=True)
    parser.add_argument("--output-json", type=Path)
    parser.add_argument("--no-restore", action="store_true")
    parser.add_argument("--max-write-ms", type=int)
    parser.add_argument("--max-restore-ms", type=int)
    parser.add_argument("--rename-random-name", action="store_true")
    parser.add_argument("--target-ble-name")
    parser.add_argument("--only-ble-name", action="store_true")
    args = parser.parse_args()

    client = CdpClient(cdp_page_ws(args.remote_debugging_port))
    try:
        before = invoke_retry(client, "get_device_settings")
        if before.get("source") != "firmware":
            raise RuntimeError(
                f"device settings source is not firmware: {before.get('source')} detail={before.get('detail')}"
            )

        seed = int(time.time())
        test_request = request_from_snapshot(before)
        if not args.only_ble_name:
            test_request.update(
                {
                    "statusLedBrightnessPercent": different(seed, int(before["statusLedBrightnessPercent"]), 31, 87),
                    "keyLedBrightnessPercent": different(seed + 11, int(before["keyLedBrightnessPercent"]), 33, 89),
                    "knobLedBrightnessPercent": different(seed + 17, int(before["knobLedBrightnessPercent"]), 35, 91),
                    "edgeLedBrightnessPercent": different(seed + 23, int(before["edgeLedBrightnessPercent"]), 37, 93),
                    "pluggedLowPowerIdleMinutes": different(
                        seed + 29,
                        int(before["pluggedLowPowerIdleMinutes"]),
                        4,
                        37,
                    ),
                    "batteryLowPowerIdleMinutes": different(
                        seed + 31,
                        int(before["batteryLowPowerIdleMinutes"]),
                        3,
                        29,
                    ),
                    "pluggedLowPowerEnabled": True,
                }
            )
        if args.rename_random_name and args.target_ble_name:
            raise RuntimeError("--rename-random-name and --target-ble-name are mutually exclusive")
        if args.rename_random_name:
            test_request["bleName"] = random_ble_name(str(before["bleName"]))
        if args.target_ble_name:
            test_request["bleName"] = args.target_ble_name

        write_started = time.perf_counter()
        written = invoke_retry(client, "set_device_settings", {"request": test_request}, attempts=1)
        write_elapsed_ms = round((time.perf_counter() - write_started) * 1000)
        confirmed = invoke_retry(client, "get_device_settings")

        fields = [
            "statusLedBrightnessPercent",
            "keyLedBrightnessPercent",
            "knobLedBrightnessPercent",
            "edgeLedBrightnessPercent",
            "pluggedLowPowerIdleMinutes",
            "batteryLowPowerIdleMinutes",
            "pluggedLowPowerEnabled",
            "bleName",
        ]
        mismatches = compare_fields(confirmed, test_request, fields)

        restore_request = request_from_snapshot(before)
        restored = None
        final_snapshot = confirmed
        restore_elapsed_ms = None
        restore_mismatches: list[dict] = []
        if not args.no_restore:
            restore_started = time.perf_counter()
            restored = invoke_retry(client, "set_device_settings", {"request": restore_request}, attempts=1)
            restore_elapsed_ms = round((time.perf_counter() - restore_started) * 1000)
            final_snapshot = invoke_retry(client, "get_device_settings")
            restore_mismatches = compare_fields(final_snapshot, before, fields)

        speed_failures = []
        if args.max_write_ms is not None and write_elapsed_ms > args.max_write_ms:
            speed_failures.append(
                {
                    "field": "writeElapsedMs",
                    "max": args.max_write_ms,
                    "actual": write_elapsed_ms,
                }
            )
        if (
            args.max_restore_ms is not None
            and restore_elapsed_ms is not None
            and restore_elapsed_ms > args.max_restore_ms
        ):
            speed_failures.append(
                {
                    "field": "restoreElapsedMs",
                    "max": args.max_restore_ms,
                    "actual": restore_elapsed_ms,
                }
            )

        result = {
            "ok": not mismatches and not restore_mismatches and not speed_failures,
            "writeElapsedMs": write_elapsed_ms,
            "restoreElapsedMs": restore_elapsed_ms,
            "before": before,
            "renamed": args.rename_random_name,
            "testRequest": test_request,
            "written": written,
            "confirmed": confirmed,
            "mismatches": mismatches,
            "restoreRequest": restore_request,
            "restored": restored,
            "finalSnapshot": final_snapshot,
            "restoreMismatches": restore_mismatches,
            "speedFailures": speed_failures,
        }
        text = json.dumps(result, ensure_ascii=False, indent=2)
        print(text)
        if args.output_json:
            args.output_json.parent.mkdir(parents=True, exist_ok=True)
            args.output_json.write_text(text + "\n", encoding="utf-8")
        return 0 if result["ok"] else 2
    finally:
        client.close()


if __name__ == "__main__":
    raise SystemExit(main())
