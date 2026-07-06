import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const benchScript = path.join(repoRoot, "scripts", "windows-listener-preproduction-bench-review.ps1");
const collectScript = path.join(repoRoot, "scripts", "windows-listener-preproduction-bench-collect.ps1");
const humanScript = path.join(repoRoot, "scripts", "windows-listener-preproduction-human-review.ps1");

const bench = fs.readFileSync(benchScript, "utf8");
const collect = fs.readFileSync(collectScript, "utf8");
const human = fs.readFileSync(humanScript, "utf8");

const requiredStepIds = [
  "baseline-type-tray-ui",
  "same-name-write-no-repair",
  "random-name-exact-cache-refresh",
  "restore-default-listener",
  "manual-windows-delete-no-type-autopair",
  "no-type-native-pairing",
  "type-takeover-no-forced-repair",
  "ec11-single-not-double",
  "ec11-double-repair-with-type",
  "computer-switch-product-flow",
  "recording-response-and-led-priority",
  "ble-audio-type-link",
  "led-independent-contract",
  "ota-wireless-smoke",
  "wired-flash-smoke",
  "release-package-final-check",
];

const requiredCapabilities = [
  "type_runtime",
  "desktop_visual_capture",
  "windows_ble_automation",
  "second_ble_host",
  "usb_power_relay",
  "power_relay",
  "physical_input_fixture",
  "led_optical_capture",
  "audio_fixture",
  "wired_flash_port",
  "ota_package",
  "release_artifacts",
];

const failures = [];

for (const stepId of requiredStepIds) {
  if (!human.includes(stepId)) {
    failures.push(`human review is missing step ${stepId}`);
  }
  if (!bench.includes(stepId)) {
    failures.push(`bench review is missing step ${stepId}`);
  }
}

for (const capability of requiredCapabilities) {
  if (!bench.includes(capability)) {
    failures.push(`bench review is missing capability ${capability}`);
  }
  if (!collect.includes(capability)) {
    failures.push(`bench evidence collector is missing capability ${capability}`);
  }
}

for (const forbidden of ["System.Windows.Forms", "MessageBox", "ShowDialog", "operator_action", "observation_template"]) {
  if (bench.includes(forbidden)) {
    failures.push(`bench review must be machine-verdict only and not contain ${forbidden}`);
  }
}

if (!bench.includes("BENCH_REVIEW_NO_GO") || !bench.includes("BENCH_REVIEW_PASS")) {
  failures.push("bench review must emit explicit BENCH_REVIEW_NO_GO/PASS states");
}

if (!bench.includes("missing_capabilities") || !bench.includes("missing_evidence")) {
  failures.push("bench review must report missing capabilities and missing evidence per step");
}

for (const requiredToken of [
  "BENCH_COLLECT_COMPLETE",
  "preproduction-bench-capabilities.json",
  "preproduction-bench-review-summary.json",
  "windows-ble-state.json",
  "release-artifacts.json",
]) {
  if (!collect.includes(requiredToken)) {
    failures.push(`bench evidence collector is missing ${requiredToken}`);
  }
}

if (collect.includes("PairAsync") || collect.includes("UnpairAsync")) {
  failures.push("passive bench evidence collection must not pair or unpair Windows devices");
}

if (failures.length > 0) {
  console.error("FAIL: preproduction bench contract check failed");
  for (const failure of failures) {
    console.error(` - ${failure}`);
  }
  process.exit(1);
}

console.log("PASS: preproduction bench contract covers all 16 release scenarios with machine capabilities and evidence.");
