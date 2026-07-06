import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const benchScript = path.join(repoRoot, "scripts", "windows-listener-preproduction-bench-review.ps1");
const collectScript = path.join(repoRoot, "scripts", "windows-listener-preproduction-bench-collect.ps1");
const activeBleScript = path.join(repoRoot, "scripts", "windows-listener-preproduction-ble-active-bench.ps1");
const humanScript = path.join(repoRoot, "scripts", "windows-listener-preproduction-human-review.ps1");
const scenarioManifestPath = path.join(repoRoot, "scripts", "listener-preproduction-scenarios.json");
const deviceSectionPath = path.join(repoRoot, "src", "pages", "settings", "DeviceSection.tsx");
const firmwareOtaPanelPath = path.join(repoRoot, "src", "pages", "settings", "FirmwareOtaPanel.tsx");
const commandsPath = path.join(repoRoot, "src-tauri", "src", "commands.rs");
const coordinatorPath = path.join(repoRoot, "src-tauri", "src", "coordinator.rs");

const bench = fs.readFileSync(benchScript, "utf8");
const collect = fs.readFileSync(collectScript, "utf8");
const activeBle = fs.readFileSync(activeBleScript, "utf8");
const human = fs.readFileSync(humanScript, "utf8");
const scenarioManifest = JSON.parse(fs.readFileSync(scenarioManifestPath, "utf8"));
const deviceSection = fs.readFileSync(deviceSectionPath, "utf8");
const firmwareOtaPanel = fs.readFileSync(firmwareOtaPanelPath, "utf8");
const commands = fs.readFileSync(commandsPath, "utf8");
const coordinator = fs.readFileSync(coordinatorPath, "utf8");

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

const mutexIndex = human.indexOf("singleInstanceMutex");
const cleanupIndex = human.indexOf("Remove-Item -LiteralPath $staleOutput");
if (mutexIndex === -1 || cleanupIndex === -1 || mutexIndex > cleanupIndex) {
  failures.push("human review must take the single-instance mutex before cleaning review outputs");
}

for (const requiredToken of ["resumeExistingFullReview", "ResumedExistingRecords"]) {
  if (!human.includes(requiredToken)) {
    failures.push(`human review must preserve unfinished operator records when the window is relaunched: ${requiredToken}`);
  }
}

for (const requiredToken of ["[string[]]$StepIds", "FocusStepIds:", "$requestedStepIds.Count -gt 0"]) {
  if (!human.includes(requiredToken)) {
    failures.push(`human review must support focused re-review without repeating already-passed steps: ${requiredToken}`);
  }
}

for (const requiredToken of ["实际操作和结果", "operator_note"]) {
  if (!human.includes(requiredToken)) {
    failures.push(`human review must use one operator note field and include ${requiredToken}`);
  }
}

for (const forbiddenToken of [
  "填写现象",
  "initialObservation",
  "现象栏还是原始模板",
  "ObservationTemplate",
  "observation_template",
  "结果/现象",
]) {
  if (human.includes(forbiddenToken)) {
    failures.push(`human review must not restore the removed separate observation field/token: ${forbiddenToken}`);
  }
}

for (const requiredToken of ["FormStartPosition]::Manual", "PrimaryScreen.WorkingArea", "$workingArea.Left + 12"]) {
  if (!human.includes(requiredToken)) {
    failures.push(`human review window must stay anchored at the lower-left operator workspace and include ${requiredToken}`);
  }
}

if (!human.includes("[System.Drawing.Size]::new(680, 500)") || !human.includes("[System.Drawing.Size]::new(640, 460)")) {
  failures.push("human review window must stay readable but bounded enough to leave Type/Windows Bluetooth visible");
}

if (!deviceSection.includes("onWheel={event => {\n          event.currentTarget.blur();\n        }}")) {
  failures.push("device minute inputs must blur on mouse wheel so scrolling the settings page cannot silently change saved minutes");
}

for (const requiredToken of [
  "const [selectedPackage, setSelectedPackage]",
  "selectedPackage && (",
  "FirmwareOtaFact label={t('settings.recording.firmwareOtaPackageVersion'",
  "FirmwareWiredFlashPanel ref={wiredRef} packagePath={selectedPackage?.path ?? null}",
  "wiredFirmwareNeedsSharedPackage",
  "firmwareOtaNeedsSharedPackage",
]) {
  if (!firmwareOtaPanel.includes(requiredToken)) {
    failures.push(`firmware OTA/wired UI must keep one visible selected package across modes: ${requiredToken}`);
  }
}

const oneClickStart = commands.indexOf("pub async fn recover_embedded_ble_device");
const oneClickEnd = commands.indexOf("#[derive(Debug, Clone, Serialize)]\n#[serde(rename_all = \"camelCase\")]\npub struct EmbeddedBleRuntimeStatus", oneClickStart);
if (oneClickStart === -1 || oneClickEnd === -1) {
  failures.push("recover_embedded_ble_device boundary must stay discoverable for the stale-pairing regression gate");
} else {
  const oneClickBody = commands.slice(oneClickStart, oneClickEnd);
  if (oneClickBody.includes("embedded_ble_windows_pairing_result(")) {
    failures.push("one-click/double-click stale cleanup must not call Type PairAsync; Windows native pairing must be user-confirmed after cleanup");
  }
  for (const requiredToken of [
    "hold_embedded_ble_listener_for_native_pairing_handoff",
    "skipped Type PairAsync after stale cleanup",
  ]) {
    if (!oneClickBody.includes(requiredToken)) {
      failures.push(`one-click recovery must hand off to Windows native pairing after Type cleanup: ${requiredToken}`);
    }
  }
}

for (const requiredToken of [
  "Type 只负责清理旧配对和等待确认，不能自己 PairAsync 抢配。",
  "如果未清旧配对就直接点连接，出现连接失败不能算通过。",
  "没有 Type 的电脑也能作为普通蓝牙键盘配对，但旧缓存必须由用户自己删除。",
]) {
  if (!human.includes(requiredToken)) {
    failures.push(`human Bluetooth acceptance must state the Type/no-Type native pairing contract: ${requiredToken}`);
  }
}

for (const requiredToken of [
  "local_stale_cache_recovery_allows_cleanup",
  "pairing.matched_devices > 0",
  "pairing.failed_devices > 0",
]) {
  if (!coordinator.includes(requiredToken)) {
    failures.push(`Type-present repair must clean local stale Windows cache evidence before native pairing: ${requiredToken}`);
  }
}

for (const requiredContract of [
  "Type must not call PairAsync",
  "Without Type, the user must manually remove stale Windows pairing",
]) {
  if (!JSON.stringify(scenarioManifest).includes(requiredContract)) {
    failures.push(`canonical scenario manifest must encode the Bluetooth repair product contract: ${requiredContract}`);
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

if (!bench.includes("missing_capabilities") || !bench.includes("missing_evidence") || !bench.includes("failed_evidence")) {
  failures.push("bench review must report missing capabilities, missing evidence, and failed evidence per step");
}

for (const requiredToken of ["timed_out", "exit_code", "*.summary.json"]) {
  if (!bench.includes(requiredToken)) {
    failures.push(`bench review must validate process-capture summary evidence health: ${requiredToken}`);
  }
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
