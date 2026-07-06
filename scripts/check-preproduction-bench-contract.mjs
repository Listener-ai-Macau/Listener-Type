import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const benchScript = path.join(repoRoot, "scripts", "windows-listener-preproduction-bench-review.ps1");
const collectScript = path.join(repoRoot, "scripts", "windows-listener-preproduction-bench-collect.ps1");
const activeBleScript = path.join(repoRoot, "scripts", "windows-listener-preproduction-ble-active-bench.ps1");
const humanScript = path.join(repoRoot, "scripts", "windows-listener-preproduction-human-review.ps1");
const scenarioManifestPath = path.join(repoRoot, "scripts", "listener-preproduction-scenarios.json");

const bench = fs.readFileSync(benchScript, "utf8");
const collect = fs.readFileSync(collectScript, "utf8");
const activeBle = fs.readFileSync(activeBleScript, "utf8");
const human = fs.readFileSync(humanScript, "utf8");
const scenarioManifest = JSON.parse(fs.readFileSync(scenarioManifestPath, "utf8"));

const scenarios = Array.isArray(scenarioManifest.scenarios) ? scenarioManifest.scenarios : [];
const requiredStepIds = scenarios.map((scenario) => scenario.id);
const requiredCapabilities = [...new Set(scenarios.flatMap((scenario) => scenario.bench_capabilities ?? []))];

const failures = [];

if (requiredStepIds.length < 18) {
  failures.push(`canonical scenario manifest must contain at least 18 release scenarios, got ${requiredStepIds.length}`);
}

for (const requiredId of ["ec11-long-press-shutdown-led", "ec11-rotate-ring-feedback"]) {
  if (!requiredStepIds.includes(requiredId)) {
    failures.push(`canonical scenario manifest is missing ${requiredId}`);
  }
}

const duplicatedStepIds = requiredStepIds.filter((id, index) => requiredStepIds.indexOf(id) !== index);
if (duplicatedStepIds.length > 0) {
  failures.push(`canonical scenario manifest has duplicate IDs: ${[...new Set(duplicatedStepIds)].join(",")}`);
}

if (!bench.includes("listener-preproduction-scenarios.json")) {
  failures.push("bench review must load the canonical scenario manifest");
}

if (!human.includes("Assert-StepsMatchCanonicalScenarios")) {
  failures.push("human review must fail fast when its detailed steps drift from the canonical scenario manifest");
}

for (const requiredToken of ["initialObservation", "现象栏还是原始模板"]) {
  if (!human.includes(requiredToken)) {
    failures.push(`human review must reject unchanged observation templates and include ${requiredToken}`);
  }
}

for (const requiredToken of ["FormStartPosition]::Manual", "PrimaryScreen.WorkingArea", "$workingArea.Left + 12"]) {
  if (!human.includes(requiredToken)) {
    failures.push(`human review window must stay anchored at the lower-left operator workspace and include ${requiredToken}`);
  }
}

for (const stepId of requiredStepIds) {
  if (!human.includes(stepId)) {
    failures.push(`human review is missing step ${stepId}`);
  }
}

for (const capability of requiredCapabilities) {
  if (!collect.includes(capability)) {
    failures.push(`bench evidence collector is missing capability ${capability}`);
  }
}

for (const scenario of scenarios) {
  if (!scenario.bench_title || !Array.isArray(scenario.bench_capabilities) || !Array.isArray(scenario.bench_evidence)) {
    failures.push(`canonical scenario ${scenario.id} must define bench_title, bench_capabilities, and bench_evidence`);
  }
}

for (const requiredToken of ["bench_capabilities", "bench_evidence"]) {
  if (!bench.includes(requiredToken)) {
    failures.push(`bench review must derive ${requiredToken} from the canonical scenario manifest`);
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
  "ActiveBleSummaryPath",
  "windows_ble_automation capability accepted",
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

for (const requiredToken of [
  "WINDOWS_BLE_AUTOMATION_DRY_RUN",
  "WINDOWS_BLE_AUTOMATION_PASS",
  "--cleanup-embedded-ble-pairing",
  "--prompt-embedded-ble-pairing-only",
  "--read-embedded-audio-ble-status",
  "ClickWindowsNotification",
  "-Execute",
]) {
  if (!activeBle.includes(requiredToken)) {
    failures.push(`active Windows BLE bench script is missing ${requiredToken}`);
  }
}

if (!activeBle.includes("PairAsync") && !activeBle.includes("--prompt-embedded-ble-pairing-only")) {
  failures.push("active Windows BLE bench script must exercise the Type pairing path");
}

if (activeBle.includes("System.Windows.Forms") || activeBle.includes("MessageBox") || activeBle.includes("ShowDialog")) {
  failures.push("active Windows BLE bench script must not use custom blocking dialog UI");
}

if (failures.length > 0) {
  console.error("FAIL: preproduction bench contract check failed");
  for (const failure of failures) {
    console.error(` - ${failure}`);
  }
  process.exit(1);
}

console.log(`PASS: preproduction bench contract covers all ${requiredStepIds.length} release scenarios with machine capabilities and evidence.`);
