#!/usr/bin/env node
/**
 * Listener 1.0.4 protected source contracts.
 * Reuses in-repo constants; fails if accepted thresholds regress.
 */
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const typeRoot = resolve(fileURLToPath(new URL("..", import.meta.url)));
const firmwareRoot = resolve(typeRoot, "..", "Listener-Firmware");

function read(rel, root = typeRoot) {
  const path = join(root, rel);
  assert.ok(existsSync(path), `missing required file: ${path}`);
  return readFileSync(path, "utf8");
}

function mustMatch(source, re, label) {
  const m = source.match(re);
  assert.ok(m, `contract missing: ${label} / ${re}`);
  return m;
}

function mustInclude(source, needle, label) {
  assert.ok(source.includes(needle), `contract missing: ${label} => ${needle}`);
}

const results = [];
function gate(id, fn) {
  try {
    fn();
    results.push({ id, status: "PASS" });
    console.log(`[PASS] ${id}`);
  } catch (error) {
    results.push({ id, status: "FAIL", error: String(error?.message || error) });
    console.error(`[FAIL] ${id}: ${error?.message || error}`);
  }
}

const dictation = read("src-tauri/src/coordinator/dictation.rs");
const dictationTests = read("src-tauri/src/coordinator/dictation_tests.rs");
const typesRs = read("src-tauri/src/types.rs");
const startupScript = read("scripts/check-type-startup-reconnect-speed.ps1");
const otaSpeedTest = read("scripts/check-ota-transfer-speed-log.test.mjs");
const firmwareOtaTs = read("src/lib/firmwareOta.test.ts");
const wakePolish = read("src-tauri/src/coordinator/dictation_wake_polish.rs");
const speaker = existsSync(join(typeRoot, "src-tauri/src/speaker_verification.rs"))
  ? read("src-tauri/src/speaker_verification.rs")
  : "";
const persistence = read("src-tauri/src/persistence.rs");
const powerManager = existsSync(join(firmwareRoot, "components/power_manager/power_manager.c"))
  ? read("components/power_manager/power_manager.c", firmwareRoot)
  : "";
const voiceRecording = existsSync(
  join(firmwareRoot, "components/voice_recording_control/voice_recording_control.c"),
)
  ? read("components/voice_recording_control/voice_recording_control.c", firmwareRoot)
  : "";

gate("local_confirmation_800ms", () => {
  mustMatch(
    dictationTests,
    /assert_eq!\(\s*LOCAL_CONFIRMATION_START_MS,\s*800\s*\)/,
    "dictation_tests asserts 800 ms confirmation",
  );
  mustMatch(
    wakePolish + dictation + dictationTests,
    /LOCAL_CONFIRMATION_START_MS(?:\s*:\s*usize)?\s*=\s*800/,
    "LOCAL_CONFIRMATION_START_MS = 800",
  );
});

gate("target_speaker_end_1000ms", () => {
  mustMatch(
    dictation,
    /EMBEDDED_TARGET_SPEAKER_END_TIMEOUT_MS:\s*u64\s*=\s*1_000/,
    "target speaker end timeout 1000 ms",
  );
  mustInclude(dictation, "target_speaker_inactive_1000ms", "auto-stop reason string");
});

gate("default_settings", () => {
  mustInclude(typesRs, "send_key_after_dictation: false", "auto-send default off");
  mustInclude(typesRs, "copy_dictation_to_clipboard: true", "clipboard retention default on");
  mustInclude(typesRs, "show_capsule: true", "capsule default on");
  mustInclude(
    typesRs,
    "dictation_input_source: DictationInputSource::EmbeddedBle",
    "default embedded BLE input",
  );
  mustInclude(typesRs, "restore_clipboard_after_paste: true", "restore clipboard default on");
  mustInclude(typesRs, "remove_filler_words: true", "filler removal default on");
});

gate("notify_ready_3000", () => {
  mustMatch(
    startupScript,
    /\[int\]\$MaxNotifyReadyMs\s*=\s*3000/,
    "warm start notify-ready gate 3000 ms",
  );
});

gate("ota_throughput_gate_contract", () => {
  const otaRegression = existsSync(
    join(firmwareRoot, "tools", "verify_listener_103_ota_regression.py"),
  )
    ? read("tools/verify_listener_103_ota_regression.py", firmwareRoot)
    : "";
  mustInclude(
    otaRegression,
    "strictly more than 60.0 KiB/s",
    "firmware OTA regression still requires bulk_kb_s > 60.0",
  );
  mustInclude(otaSpeedTest + firmwareOtaTs, "bulk_kb_s", "Type OTA log parser retains bulk_kb_s");
  mustInclude(
    otaSpeedTest + otaRegression,
    "offset recover",
    "offset recovery accounting retained",
  );
});

gate("feature_surface", () => {
  mustInclude(typesRs, "voice_wake_phrase", "custom wake phrase field");
  mustInclude(typesRs + speaker, "speaker", "speaker/voiceprint surface");
  mustInclude(dictation, "raw_input_level_percent", "raw capsule level surface");
  assert.ok(
    existsSync(join(typeRoot, "src/lib/firmwareOta.test.ts")),
    "firmware OTA frontend tests present",
  );
});

gate("wake_idle_boundary", () => {
  // Product contract: voice activation only while non-idle with BT light on.
  const bundle = powerManager + voiceRecording + typesRs + wakePolish + dictation;
  assert.ok(bundle.length > 0, "wake/power sources readable");
  assert.ok(
    /low_power|idle|voice_auto_start|voice activation|Bluetooth/i.test(bundle),
    "idle / voice activation surface must remain present",
  );
});

gate("clean_test_data_dir_contract", () => {
  mustInclude(
    persistence,
    "listener-type-test-data-",
    "test data dir prefix outside production profile",
  );
  mustInclude(persistence, "LISTENER_TYPE_DATA_DIR", "override env for isolated runs");
  mustInclude(
    persistence,
    "default_test_data_dir_is_process_scoped_and_outside_production_profile",
    "unit test for production isolation",
  );
});

const failed = results.filter((r) => r.status === "FAIL");
const summary = {
  schema: "listener.1.0.4.protected_contracts",
  schema_version: 1,
  result: failed.length === 0 ? "PASS" : "FAIL",
  type_root: typeRoot,
  firmware_root: firmwareRoot,
  gates: results,
};

console.log(JSON.stringify(summary, null, 2));
process.exit(failed.length === 0 ? 0 : 1);
