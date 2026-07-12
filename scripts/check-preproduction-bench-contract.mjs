import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const benchScript = path.join(repoRoot, "scripts", "windows-listener-preproduction-bench-review.ps1");
const collectScript = path.join(repoRoot, "scripts", "windows-listener-preproduction-bench-collect.ps1");
const activeBleScript = path.join(repoRoot, "scripts", "windows-listener-preproduction-ble-active-bench.ps1");
const focusedBleHumanScript = path.join(repoRoot, "scripts", "windows-ble-focused-human-review.ps1");
const humanScript = path.join(repoRoot, "scripts", "windows-listener-preproduction-human-review.ps1");
const operatorNoteTriageScript = path.join(repoRoot, "scripts", "check-preproduction-operator-note-triage.mjs");
const totalReviewAdvanceScript = path.join(repoRoot, "scripts", "advance-preproduction-total-review.ps1");
const scenarioManifestPath = path.join(repoRoot, "scripts", "listener-preproduction-scenarios.json");
const deviceSectionPath = path.join(repoRoot, "src", "pages", "settings", "DeviceSection.tsx");
const firmwareOtaPanelPath = path.join(repoRoot, "src", "pages", "settings", "FirmwareOtaPanel.tsx");
const commandsPath = path.join(repoRoot, "src-tauri", "src", "commands.rs");
const coordinatorPath = path.join(repoRoot, "src-tauri", "src", "coordinator.rs");

const bench = fs.readFileSync(benchScript, "utf8");
const collect = fs.readFileSync(collectScript, "utf8");
const activeBle = fs.readFileSync(activeBleScript, "utf8");
const focusedBleHuman = fs.readFileSync(focusedBleHumanScript, "utf8");
const human = fs.readFileSync(humanScript, "utf8");
const operatorNoteTriage = fs.readFileSync(operatorNoteTriageScript, "utf8");
const totalReviewAdvance = fs.existsSync(totalReviewAdvanceScript)
  ? fs.readFileSync(totalReviewAdvanceScript, "utf8")
  : "";
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

for (const requiredId of ["pwr-boot-shutdown-led", "ec11-long-press-shutdown-led", "ec11-rotate-ring-feedback"]) {
  if (!requiredStepIds.includes(requiredId)) {
    failures.push(`canonical scenario manifest is missing ${requiredId}`);
  }
}

const pwrScenario = scenarios.find((scenario) => scenario.id === "pwr-boot-shutdown-led");
const pwrContract = pwrScenario?.product_contract ?? "";
if (
  pwrContract.includes("white full-brightness") ||
  human.includes("white full-brightness cue") ||
  human.includes("开机白灯")
) {
  failures.push("PWR boot human/manifest contract must not regress to the rejected white boot light");
}
for (const requiredToken of ["warm amber", "与关机确认同色同亮", "Type 状态灯四区亮度 cap"]) {
  if (!pwrContract.includes(requiredToken) && !human.includes(requiredToken)) {
    failures.push(`PWR boot acceptance must require shutdown-matching capped amber: ${requiredToken}`);
  }
}
for (const requiredToken of ["startup time is measured", "stable BLE-visible readiness", "no startup-complete/PWR-normal cue may be shown early", "不能为了显得启动更快而提前显示"]) {
  if (!pwrContract.includes(requiredToken) && !human.includes(requiredToken)) {
    failures.push(`PWR boot acceptance must forbid fake startup-time optimization: ${requiredToken}`);
  }
}

const scenarioById = new Map(scenarios.map((scenario) => [scenario.id, scenario]));
const scenarioContract = (id) => scenarioById.get(id)?.product_contract ?? "";
for (const [stepId, requiredTokens] of [
  [
    "same-name-write-no-repair",
    ["current BLE name", "no DEVICE:SET ble_name", "no BLE-name apply", "no Windows cache refresh", "no PairAsync", "no pairing prompt"],
  ],
  [
    "random-name-exact-cache-refresh",
    ["different valid BLE name", "update firmware", "same Type-controlled recovery path", "same BLE blue double-flash/reconnect recovery cue", "without opening this PC's Windows Bluetooth Settings/user pairing prompt", "exact new fully random ASCII name", "no listener/LT/type family prefix", "silent rename recovery", "other computers do not receive a Swift Pair prompt during rename", "Explicit cross-computer re-pair remains the double-click/native Windows pairing flow", "Same-name writes remain no-repair/no-prompt"],
  ],
  [
    "restore-default-listener",
    ["default BLE name listener", "same Type-controlled cache refresh/reconnect path", "same BLE blue double-flash/reconnect recovery cue", "avoid opening this PC's Windows Bluetooth Settings/user pairing prompt", "suppress Swift Pair prompts on other computers during rename", "exact display-name confirmation"],
  ],
  [
    "no-type-native-pairing",
    ["Listener Type fully exited", "no listener-type.exe process", "Windows native Add device > Bluetooth", "without Type PairAsync", "BTHLE listener OK", "HID service 00001812", "keyboard.inf/kbdhid Col01", "looping between connected and disconnected"],
  ],
  [
    "type-takeover-no-forced-repair",
    ["latest installed MSI Listener Type", "existing paired listener", "without PairAsync", "Windows pairing prompt", "forced repair", "device deletion", "C:\\Program Files\\Listener Type\\listener-type.exe", "BTHLE listener OK", "background BLE capture", "background listener notify ready within 3 seconds", "persisted startup path", "device-key recording/control events"],
  ],
]) {
  const contract = scenarioContract(stepId);
  for (const token of requiredTokens) {
    if (!contract.includes(token)) {
      failures.push(`scenario ${stepId} must encode product contract token: ${token}`);
    }
  }
}

for (const requiredToken of [
  "这个步骤是同名写入，不是改名。",
  "不能触发重新配对。",
  "任意合法 1-29 个可见 ASCII 名字都能写入",
  "本步骤生成的随机名必须是无产品前缀的 12 位随机 ASCII 串",
  "不同名改名由 Type 自动清理本机旧配对并恢复",
  "BLE 灯效和双击恢复一样",
  "改名恢复走 silent Type 路径",
  "其它电脑也不应因为改名收到 Swift Pair 弹窗",
  "不能继续显示旧缓存名",
  "默认名字 listener 能恢复。",
]) {
  if (!human.includes(requiredToken)) {
    failures.push(`BLE rename human acceptance must protect same-name/random/default behavior: ${requiredToken}`);
  }
}
if (
  !human.includes("function New-ReviewRandomBleName") ||
  !human.includes("[System.Security.Cryptography.RandomNumberGenerator]::GetInt32") ||
  !human.includes("$i -lt 12") ||
  !human.includes("(?i:listener|listner|lt|type)") ||
  human.includes("Get-Random -Count 8") ||
  human.includes('$RandomName = "listener-$suffix"') ||
  human.includes('$RandomName = "LT$suffix"')
) {
  failures.push("random BLE name human review must generate a fully random 12-character ASCII name without listener/listner/LT/type family prefixes by default");
}

for (const requiredToken of [
  "device_settings_update_commands_skip_unchanged_ble_name",
  "device_ble_name_change_path_refreshes_windows_cache_after_apply",
  "device_ble_name_change_serial_batch_only_writes_name",
  "ble_name_refresh_uses_silent_recovery_while_one_click_keeps_user_prompt",
  "device_ble_name_same_name_no_repair_hardware_smoke",
  "device_ble_name_windows_refresh_roundtrip_hardware_smoke",
]) {
  if (!commands.includes(requiredToken)) {
    failures.push(`BLE rename Rust regression coverage is missing: ${requiredToken}`);
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

for (const requiredToken of [
  "Listener 1.0.2 总验收",
  "总体验收范围",
  "当前总进度",
  "$overallIndexById",
  "$progressText",
  "$($ReviewScope)第",
  "有备注会停下修备注",
  "progress = [ordered]@",
  "One overall acceptance session shows total scope and progress",
]) {
  if (!human.includes(requiredToken)) {
    failures.push(`human review must show overall acceptance progress while advancing one focused item at a time: ${requiredToken}`);
  }
}
if (human.includes("$ReviewScope第")) {
  failures.push("human review overall progress label must use $($ReviewScope) before Chinese suffixes to avoid PowerShell variable-name expansion");
}

for (const requiredToken of [
  "focused_review_status",
  "FOCUSED_HUMAN_REVIEW_PASS",
  "focused_human_review_status=",
  '$focusedReviewStatus -eq "FOCUSED_HUMAN_REVIEW_PASS"',
  "Get-FocusedReviewStatus",
  "StatusSelfTest",
]) {
  if (!human.includes(requiredToken)) {
    failures.push(`focused human review must let a selected PASS step complete without claiming full release acceptance: ${requiredToken}`);
  }
}

for (const requiredToken of [
  "Get-ExplicitTotalReviewRecordSet",
  "TotalReviewStatePath",
  "Get-NormalizedExistingPath",
  "carried_forward_summary",
  "carried_forward_count",
  "carried_forward_sources",
  "carried_forward_blocked_items",
  "total_review_state",
  "total_review_next_step_id",
  "Current total review state",
  "carried forward from previous PASS summary",
]) {
  if (!human.includes(requiredToken)) {
    failures.push(`focused human review must merge old PASS records instead of forcing all canonical steps to be repeated: ${requiredToken}`);
  }
}

for (const requiredToken of [
  "total_review_advance_script",
  "advance-preproduction-total-review.ps1",
  '$copy["operator_note"] = ""',
  "carried_forward_operator_note_status",
]) {
  if (!human.includes(requiredToken)) {
    failures.push(`focused human review must close carried-forward notes and expose the guarded total-review advance path: ${requiredToken}`);
  }
}
for (const requiredToken of [
  "Focused review triage is required because the summary contains an operator note",
  "check-preproduction-operator-note-triage.mjs",
  "Move-Item -LiteralPath $tempPath -Destination $statePath -Force",
  "last_advanced_summary",
  "next_step_id",
]) {
  if (!totalReviewAdvance.includes(requiredToken)) {
    failures.push(`total-review advance guard is missing ${requiredToken}`);
  }
}

for (const requiredToken of ["ExecutablePath", "FileVersion", "ProductVersion", "Sha256", "Get-FileHash"]) {
  if (!human.includes(requiredToken)) {
    failures.push(`human review process evidence must prove which Type exe was tested: ${requiredToken}`);
  }
  if (!collect.includes(requiredToken)) {
    failures.push(`bench process evidence must prove which Type exe was tested: ${requiredToken}`);
  }
}

const baselineTypeContract = scenarioContract("baseline-type-tray-ui");
for (const requiredToken of [
  "latest MSI-updated user installation",
  "Program Files listener-type.exe",
  "desktop Listener Type.lnk",
  "not a repo build output",
  "ExecutablePath, FileVersion, ProductVersion, and Sha256",
]) {
  if (!baselineTypeContract.includes(requiredToken)) {
    failures.push(`baseline Type tray/UI scenario must require installed-MSI user path evidence: ${requiredToken}`);
  }
}

if (!baselineTypeContract.includes("ExecutablePath, FileVersion, ProductVersion, and Sha256")) {
  failures.push("baseline Type scenario must keep installed exe identity evidence in the product contract");
}

for (const requiredToken of [
  "Set-TypeWindowForeground",
  "Select-TypeCaptureWindow",
  "no_visible_type_window",
  "Start-TypeWindowForBench",
  "type-window-capture.json",
  "desktop screenshot captured, but Type window focus failed",
  "desktop_visual_capture = (",
]) {
  if (!collect.includes(requiredToken)) {
    failures.push(`bench collector must bind baseline screenshot evidence to the foreground Type window: ${requiredToken}`);
  }
}

for (const requiredToken of [
  "type_window_capture",
  "type_window_not_focused",
  "missing_type_process_id",
]) {
  if (!bench.includes(requiredToken)) {
    failures.push(`bench review must validate Type window focus metadata: ${requiredToken}`);
  }
}

for (const requiredToken of [
  "先用最新 MSI 更新 Listener Type",
  "C:\\Program Files\\Listener Type\\listener-type.exe",
  "Listener Type.lnk",
  "不要从 repo target 目录启动",
  "桌面快捷方式和托盘启动都指向已安装的最新 MSI exe",
]) {
  if (!human.includes(requiredToken)) {
    failures.push(`human baseline Type acceptance must force the installed-MSI user path: ${requiredToken}`);
  }
}

for (const requiredToken of ["type-brightness-low-power-sync", "从 Windows 托盘图标打开最新 Type"]) {
  if (!human.includes(requiredToken)) {
    failures.push(`Type brightness/low-power acceptance must require testing the latest tray Type: ${requiredToken}`);
  }
}
const typeBrightnessContract = scenarioContract("type-brightness-low-power-sync");
for (const requiredToken of ["cleared and retyped", "leading-zero value such as 050"]) {
  if (!typeBrightnessContract.includes(requiredToken)) {
    failures.push(`Type brightness/low-power scenario must protect numeric input editing: ${requiredToken}`);
  }
}
for (const requiredToken of ["清空任意一个亮度数字框后直接输入 50", "不能残留前导 0 变成 050"]) {
  if (!human.includes(requiredToken)) {
    failures.push(`Type brightness/low-power human acceptance must cover leading-zero numeric input regression: ${requiredToken}`);
  }
}

for (const requiredToken of [
  "C:\\Program Files\\Listener Type\\listener-type.exe",
  "$TypeExe = \"C:\\Program Files\\Listener Type\\listener-type.exe\"",
]) {
  if (!collect.includes(requiredToken)) {
    failures.push(`bench collection must default to the installed MSI Type: ${requiredToken}`);
  }
  if (!activeBle.includes(requiredToken)) {
    failures.push(`active BLE bench must default to the installed MSI Type: ${requiredToken}`);
  }
}

for (const forbiddenToken of [
  "src-tauri\\target\\x86_64-pc-windows-msvc\\release\\listener-type.exe",
  "src-tauri\\target\\release\\listener-type.exe",
]) {
  if (collect.includes(forbiddenToken)) {
    failures.push(`bench collection must not silently fall back to repo build outputs: ${forbiddenToken}`);
  }
  if (activeBle.includes(forbiddenToken)) {
    failures.push(`active BLE bench must not silently fall back to repo build outputs: ${forbiddenToken}`);
  }
}

for (const requiredToken of [
  'preproduction-human-review*',
  'operator-note", "dryrun", "dry-run", "smoke", "fixture", "script-gate", "format-check", "no-hardware"',
  "Summary.output_dir",
  "Summary.session_jsonl",
  "preproduction-operator-note-triage.json",
  "triage.source_summary",
  "operatorNoteProperty",
  "triageSourceProperty",
  "carriedForwardProperty",
  "completed_records",
  'HUMAN_REVIEW_INCOMPLETE',
]) {
  if (!human.includes(requiredToken)) {
    failures.push(`focused human review carry-forward must reject fixture/dry-run summaries and allow only triaged real human records: ${requiredToken}`);
  }
}

for (const requiredToken of [
  "operator_note_review_status",
  "operator_note_review_required_count",
  "preproduction-operator-note-triage.template.json",
  "Blank operator_note means the step had no extra operator remarks",
  "Every non-empty operator note is treated as a human prompt",
  "operator_note_acknowledged",
  "parsed_requests",
]) {
  if (!human.includes(requiredToken)) {
    failures.push(`human review must treat every operator note as a prompt requiring triage: ${requiredToken}`);
  }
}

for (const requiredToken of [
  "operator_note_requires_triage=1",
  "human_review_stopped_after_operator_note=1",
  "stopped_after_operator_note",
  "-or $stoppedAfterOperatorNote",
  "Stopped after operator note",
]) {
  if (!human.includes(requiredToken)) {
    failures.push(`human review must stop the current review after any non-empty operator note: ${requiredToken}`);
  }
}

for (const forbiddenToken of [
  'operator_note = "NoPrompt dry run"',
  'operator_action = "NoPrompt dry run"',
  'observation = "NoPrompt dry run"',
]) {
  if (human.includes(forbiddenToken)) {
    failures.push(`dry-run sentinels must not be written into human operator note fields: ${forbiddenToken}`);
  }
}

for (const requiredToken of ['dry_run_note = "NoPrompt dry run"', 'text === "NoPrompt dry run"']) {
  if (!human.includes(requiredToken) && !operatorNoteTriage.includes(requiredToken)) {
    failures.push(`dry-run NoPrompt placeholders must stay internal and ignored by operator-note gates: ${requiredToken}`);
  }
}

for (const requiredToken of [
  "triage status must be PASS",
  "missing triage for",
  "fixed",
  "accepted_benign",
  "deferred_by_human",
  "operator_note_acknowledged=true",
  "parsed_requests",
  "blank notes are treated as normal pass",
]) {
  if (!operatorNoteTriage.includes(requiredToken)) {
    failures.push(`operator note triage gate must reject unreviewed human notes: ${requiredToken}`);
  }
}

for (const requiredToken of [
  "isReleaseCandidateSummary",
  "normalizedExistingPath",
  "pathLooksSynthetic",
  "hasDryRunRecord",
  "dry_run_note",
  "summary.output_dir",
  "summary.session_jsonl",
]) {
  if (!operatorNoteTriage.includes(requiredToken)) {
    failures.push(`operator note triage gate must ignore NoPrompt/fixture summaries when choosing latest release evidence: ${requiredToken}`);
  }
}

for (const requiredToken of ["备注（可留空", "我会当作需求处理", "operator_note"]) {
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

for (const requiredToken of ["FormStartPosition]::Manual", "PrimaryScreen.WorkingArea", "$workingArea.Left + 12", "$workingArea.Top + 72"]) {
  if (!human.includes(requiredToken)) {
    failures.push(`human review window must stay anchored inside the operator workspace and include ${requiredToken}`);
  }
}

for (const requiredToken of [
  "[System.Drawing.Size]::new(680, 360)",
  "[System.Drawing.Size]::new(640, 340)",
  "$form.AutoScroll = $false",
  "$buttonPanel.Location = [System.Drawing.Point]::new(16, 318)",
]) {
  if (!human.includes(requiredToken)) {
    failures.push(`human review window must stay compact and keep its buttons visible on the operator screen: ${requiredToken}`);
  }
}

if (!deviceSection.includes("onWheel={event => {\n          event.currentTarget.blur();\n        }}")) {
  failures.push("device minute inputs must blur on mouse wheel so scrolling the settings page cannot silently change saved minutes");
}

for (const requiredToken of [
  "const [selectedPackage, setSelectedPackage]",
  "selectedPackageAriaLabel",
  "firmwareOtaReselectPackage",
  "selectedPackage && (",
  "className=\"ol-firmware-selected-package\"",
  "onClick={firmwareActionBusy ? undefined : () => void choosePackage(selectedPackage.sourceKind === 'directory')}",
  "title={selectedPackage.path}",
  "firmwareSelectedPackage",
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
  if (!oneClickBody.includes("embedded_ble_windows_pairing_result(")) {
    failures.push("one-click/double-click stale cleanup must run the bounded Type PairAsync recovery path when Type is present");
  }
  for (const requiredToken of [
    "hold_embedded_ble_listener_for_native_pairing_handoff",
    "will stop if another host completes pairing before this Type instance",
  ]) {
    if (!oneClickBody.includes(requiredToken)) {
      failures.push(`one-click recovery must hold Type BLE and stop if another host wins pairing: ${requiredToken}`);
    }
  }
}

for (const requiredToken of [
  "Type 会先清理本机旧配对，再寻找恢复广播并走本机自动 PairAsync/GATT 恢复。",
  "如果另一台电脑先用 Windows 弹窗连上，本机 Type 不能循环清理或抢回。",
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
    failures.push(`Type-present repair must clean local stale Windows cache evidence before automatic PairAsync recovery: ${requiredToken}`);
  }
}

for (const requiredContract of [
  "first clears this PC's stale Windows pairing cache",
  "fresh Listener recovery advertisement",
  "bounded Type PairAsync recovery path automatically",
  "another host pairs first",
  "Without Type, the user must manually remove stale Windows pairing",
]) {
  if (!JSON.stringify(scenarioManifest).includes(requiredContract)) {
    failures.push(`canonical scenario manifest must encode the Bluetooth repair product contract: ${requiredContract}`);
  }
}

const releaseScenario = scenarioById.get("release-package-final-check");
for (const requiredToken of [
  "source_hash_comparison",
  "root MSI hash must match the latest Type MSI source artifact",
  "root firmware OTA zip hash must match the latest stable firmware OTA source artifact",
  "no Type portable zip or older package",
]) {
  if (!JSON.stringify(releaseScenario ?? {}).includes(requiredToken)) {
    failures.push(`release package final scenario must encode source-hash root staging contract: ${requiredToken}`);
  }
}
for (const requiredToken of [
  "SHA256 必须分别等于本次最终打包源产物",
  "type_source_hash_matches_root",
  "firmware_source_hash_matches_root",
]) {
  if (!human.includes(requiredToken)) {
    failures.push(`release package human review must require source-to-root hash match: ${requiredToken}`);
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
  "all_latest_sources_staged",
  "forbidden_root_packages",
  "type_source_hash_matches_root",
  "firmware_source_hash_matches_root",
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

for (const requiredToken of [
  "type-exit-led-clears",
  "从托盘退出 Listener Type",
  "2 秒内进程应退出",
  "等待到 12 秒",
  "是否残留 listener-type.exe",
]) {
  if (!focusedBleHuman.includes(requiredToken)) {
    failures.push(`focused BLE human review must capture tray-exit speed and Type-ready clear evidence: ${requiredToken}`);
  }
}

if (failures.length > 0) {
  console.error("FAIL: preproduction bench contract check failed");
  for (const failure of failures) {
    console.error(` - ${failure}`);
  }
  process.exit(1);
}

console.log(`PASS: preproduction bench contract covers all ${requiredStepIds.length} release scenarios with machine capabilities and evidence.`);
