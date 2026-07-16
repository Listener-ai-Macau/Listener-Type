#!/usr/bin/env node
import { readFileSync } from "node:fs";
import { join } from "node:path";

const repoRoot = process.cwd();
const baselinePath = join(repoRoot, "scripts", "performance-baselines.json");
const baselines = JSON.parse(readFileSync(baselinePath, "utf8"));
const packageJson = JSON.parse(readFileSync(join(repoRoot, "package.json"), "utf8"));

function fail(message) {
  throw new Error(message);
}

function requireNumber(value, label) {
  if (!Number.isFinite(value)) fail(`${label} must be a finite number`);
}

if (baselines.schema_version !== 1) {
  fail("performance baseline schema_version must be 1");
}

const contracts = baselines.contracts ?? {};
for (const key of [
  "device_settings_compact_write",
  "recording_low_latency",
  "ble_rename_recovery",
  "type_takeover_no_forced_repair",
  "ec11_type_repair_recovery",
  "ota_transfer_speed",
]) {
  if (!contracts[key]) fail(`missing performance baseline contract: ${key}`);
}

const settings = contracts.device_settings_compact_write;
if (settings.accepted !== true) fail("device settings speed baseline must stay accepted");
for (const key of [
  "measured_write_elapsed_ms",
  "measured_restore_elapsed_ms",
  "max_write_elapsed_ms",
  "max_restore_elapsed_ms",
]) {
  requireNumber(settings[key], `device settings ${key}`);
}
if (settings.max_write_elapsed_ms > 2500) {
  fail("device settings max write latency must not drift above 2500 ms");
}
if (settings.max_restore_elapsed_ms > 3000) {
  fail("device settings max restore latency must not drift above 3000 ms");
}
if (!settings.validation_command?.includes("--max-write-ms 2500")) {
  fail("device settings installed e2e command must enforce the write latency ceiling");
}

const settingsE2e = readFileSync(
  join(repoRoot, "scripts", "windows-installed-device-settings-write-e2e.py"),
  "utf8",
);
for (const token of [
  "--max-write-ms",
  "--max-restore-ms",
  "--rename-random-name",
  "speedFailures",
  "writeElapsedMs",
  "restoreElapsedMs",
]) {
  if (!settingsE2e.includes(token)) {
    fail(`installed device settings e2e must keep speed enforcement token: ${token}`);
  }
}

const recording = contracts.recording_low_latency;
if (recording.accepted !== true) fail("recording speed baseline must stay accepted");
for (const key of [
  "measured_max_start_to_preview_ms",
  "measured_max_submit_to_preview_ms",
  "measured_max_submitted_to_done_ms",
  "max_start_to_preview_ms",
  "max_submit_to_preview_ms",
  "max_submitted_to_done_ms",
]) {
  requireNumber(recording[key], `recording ${key}`);
}
if (recording.max_start_to_preview_ms > 1800) {
  fail("recording start-to-preview latency ceiling must not drift above 1800 ms");
}
if (recording.max_submit_to_preview_ms > 2000) {
  fail("recording submit-to-preview latency ceiling must not drift above 2000 ms");
}
if (recording.max_submitted_to_done_ms > 1200) {
  fail("recording submitted-to-done latency ceiling must not drift above 1200 ms");
}
if (!recording.evidence?.includes("asr-latency") || !recording.evidence?.endsWith("run-summary.json")) {
  fail("recording speed baseline evidence must cite an ASR latency run-summary artifact");
}
for (const key of ["all_force_raw", "no_streaming_polish_or_401", "no_transport_errors"]) {
  if (recording[key] !== true) {
    fail(`recording speed baseline must keep ${key}=true`);
  }
}
for (const token of [
  "run_embedded_audio_file_smoke.ps1",
  "Program Files\\Listener Type\\listener-type.exe",
  "-UseExistingInstance",
  "force raw",
  "no streaming polish/401",
]) {
  if (!recording.validation_command?.includes(token)) {
    fail(`recording validation command must preserve token: ${token}`);
  }
}

const rename = contracts.ble_rename_recovery;
if (rename.accepted !== true) fail("BLE rename speed baseline must stay accepted");
for (const key of [
  "measured_random_rename_elapsed_ms",
  "measured_restore_elapsed_ms",
  "max_random_rename_elapsed_ms",
  "max_restore_elapsed_ms",
]) {
  requireNumber(rename[key], `BLE rename ${key}`);
}
if (rename.max_random_rename_elapsed_ms > 10000) {
  fail("BLE random rename latency ceiling must not drift above 10000 ms");
}
if (rename.max_restore_elapsed_ms > 10000) {
  fail("BLE rename restore latency ceiling must not drift above 10000 ms");
}
if (rename.measured_random_rename_elapsed_ms > rename.max_random_rename_elapsed_ms) {
  fail("accepted BLE random rename evidence exceeds its latency ceiling");
}
if (rename.measured_restore_elapsed_ms > rename.max_restore_elapsed_ms) {
  fail("accepted BLE rename restore evidence exceeds its latency ceiling");
}
if (!rename.validation_command?.includes("windows-installed-device-settings-ui-e2e.py")) {
  fail("BLE rename recovery command must use the visible installed Type UI probe");
}
if (!rename.validation_command?.includes("--different-random-name-roundtrip")) {
  fail("BLE rename recovery command must run the full random-name rename path");
}
if (!rename.validation_command?.includes("--max-rename-total-ms 10000")) {
  fail("BLE rename recovery command must enforce the 10000 ms rename ceiling");
}
if (!rename.validation_command?.includes("notify ready <=10000 ms")) {
  fail("BLE rename recovery command must measure completion at notify ready");
}

const takeover = contracts.type_takeover_no_forced_repair;
if (takeover.accepted !== true) {
  fail("Type takeover speed baseline must stay accepted after human acceptance");
}
for (const key of [
  "measured_start_to_gatt_ready_ms",
  "measured_start_to_selected_device_ms",
  "measured_start_to_notify_ready_ms",
  "measured_start_to_type_heartbeat_ready_ms",
  "max_start_to_notify_ready_ms",
]) {
  requireNumber(takeover[key], `Type takeover ${key}`);
}
if (takeover.max_start_to_notify_ready_ms > 3000) {
  fail("Type restart notify-ready latency ceiling must not drift above 3000 ms");
}
if (!takeover.human_acceptance_evidence?.includes("type-takeover-no-forced-repair")) {
  fail("Type takeover baseline must retain the accepted focused human review evidence");
}
if (!takeover.evidence?.includes("type-startup-reconnect") || !takeover.evidence?.endsWith("summary.json")) {
  fail("Type restart speed baseline evidence must cite a startup reconnect summary artifact");
}
if (takeover.persisted_path_required !== true) {
  fail("Type restart speed baseline must require the persisted startup path");
}
if (takeover.no_pair_async !== true) {
  fail("Type restart speed baseline must keep no_pair_async=true");
}
for (const token of [
  "check-type-startup-reconnect-speed.ps1",
  "-RequirePersistedPath",
  "-MaxNotifyReadyMs 3000",
  "no PairAsync",
  "persisted_path_used=true",
  "background listener notify ready <=3000 ms",
]) {
  if (!takeover.validation_command?.includes(token)) {
    fail(`Type takeover validation command must preserve token: ${token}`);
  }
}

const ec11Recovery = contracts.ec11_type_repair_recovery;
if (ec11Recovery.accepted !== true) {
  fail("EC11 Type recovery speed baseline must stay accepted after human acceptance");
}
for (const key of [
  "measured_recovery_trigger_to_notify_ready_ms",
  "measured_recovery_trigger_to_type_heartbeat_ready_ms",
  "measured_human_disconnect_to_type_heartbeat_ready_ms",
  "measured_human_recovery_adv_to_notify_ready_ms",
  "max_recovery_trigger_to_notify_ready_ms",
  "max_recovery_advertisement_to_notify_ready_ms",
  "machine_randomized_sample_count",
  "machine_randomized_observed_max_trigger_to_type_ready_ms",
]) {
  requireNumber(ec11Recovery[key], `EC11 Type recovery ${key}`);
}
const ownerSpeedAlignment = ec11Recovery.owner_speed_alignment;
if (!ownerSpeedAlignment || !["active", "accepted"].includes(ownerSpeedAlignment.status)) {
  fail("EC11 Type recovery must retain an active or human-accepted owner decision aligning it with rename recovery");
}
if (ownerSpeedAlignment.status === "accepted") {
  for (const key of ["accepted_at", "human_acceptance_evidence", "machine_acceptance_evidence"]) {
    if (typeof ownerSpeedAlignment[key] !== "string" || ownerSpeedAlignment[key].trim() === "") {
      fail(`accepted EC11 owner speed alignment must retain ${key}`);
    }
  }
  for (const key of [
    "measured_type_notice_to_notify_ready_ms",
    "measured_recovery_advertisement_to_notify_ready_ms",
  ]) {
    requireNumber(ownerSpeedAlignment[key], `accepted EC11 owner speed alignment ${key}`);
    if (ownerSpeedAlignment[key] > 10000) {
      fail(`accepted EC11 owner speed alignment ${key} exceeds the 10000 ms rename-recovery target`);
    }
  }
}
if (ec11Recovery.max_recovery_trigger_to_notify_ready_ms !== 10000) {
  fail("EC11 Type recovery notify-ready latency ceiling must stay at the strict 10000 ms rename-recovery target");
}
if (ec11Recovery.max_recovery_advertisement_to_notify_ready_ms !== 10000) {
  fail("EC11 recovery-advertisement to notify-ready ceiling must stay at the strict 10000 ms rename-recovery target");
}
if (ec11Recovery.machine_randomized_sample_count < 5) {
  fail("EC11 Type recovery machine gate must retain at least five randomized samples");
}
if (
  ec11Recovery.machine_randomized_observed_max_trigger_to_type_ready_ms >
  ec11Recovery.max_recovery_trigger_to_notify_ready_ms
) {
  fail("EC11 Type recovery randomized machine evidence exceeds the strict notify-ready target");
}
for (const key of [
  "machine_speed_gate_requires_all_samples",
  "machine_speed_gate_requires_firmware_pre_reset_notice",
  "machine_speed_gate_requires_type_pre_reset_notice",
]) {
  if (ec11Recovery[key] !== true) {
    fail("EC11 Type recovery machine speed gate must keep " + key + "=true");
  }
}
if (ec11Recovery.shared_type_recovery_path_required !== true) {
  fail("EC11 Type recovery must stay on the shared Type PairAsync/GATT/notify path");
}
if (ec11Recovery.type_observed_direct_pairasync_required !== true) {
  fail("EC11 Type recovery must use the Type-observed recovery address before slow scans");
}
if (ec11Recovery.no_slow_recovery_paths_after_observed_address !== true) {
  fail("EC11 Type recovery must keep slow recovery paths disabled after observed-address evidence");
}
if (ec11Recovery.no_duplicate_advertisement_scan_after_notify_evidence !== true) {
  fail("EC11 Type recovery must not repeat an advertisement scan after notify-open evidence");
}
if (ec11Recovery.single_click_no_double !== true) {
  fail("EC11 single-click boundary must stay protected against double-click recovery regression");
}
if (ec11Recovery.human_acceptance_step_id !== "ec11-double-repair-with-type") {
  fail("EC11 Type recovery baseline must retain the focused human acceptance step id");
}
if (
  !ec11Recovery.human_acceptance_evidence?.includes("ec11-boundary-20260710-20s") ||
  !ec11Recovery.human_acceptance_evidence?.endsWith("human-ec11-boundary-type-recovery-rootfix.txt")
) {
  fail("EC11 Type recovery baseline must cite the focused root-fix human acceptance note");
}
if (
  !ec11Recovery.evidence?.includes("ec11-double-root-capture-20260714") ||
  !ec11Recovery.evidence?.endsWith("randomized-phase-machine-speed-20260714-2120/summary.json")
) {
  fail("EC11 Type recovery baseline must cite the randomized machine speed summary");
}
if (
  !ec11Recovery.type_log_evidence?.includes("ec11-double-root-capture-20260714") ||
  !ec11Recovery.type_log_evidence?.endsWith("type-log-pre-reset-notice-slice.txt")
) {
  fail("EC11 Type recovery baseline must cite the pre-reset notice Type log slice");
}
if (
  !ec11Recovery.historical_firmware_double_click_evidence?.includes("ec11-boundary-20260710-20s") ||
  !ec11Recovery.historical_firmware_double_click_evidence?.endsWith("firmware-ec11-double-observed-address.log") ||
  !ec11Recovery.historical_type_log_evidence?.includes("ec11-boundary-20260710-20s") ||
  !ec11Recovery.historical_type_log_evidence?.endsWith("type-log-after-human-rootfix-recovery-slice.txt")
) {
  fail("EC11 Type recovery baseline must retain its pre-fix accepted diagnostic evidence");
}
if (
  !ec11Recovery.single_click_boundary_evidence?.includes("ec11-boundary-20260710-20s") ||
  !ec11Recovery.single_click_boundary_evidence?.endsWith("firmware-ec11-single-start-stop-observed-address.log")
) {
  fail("EC11 Type recovery baseline must cite the single-click start/stop boundary artifact");
}
if (ec11Recovery.measured_recovery_trigger_to_type_heartbeat_ready_ms > 20000) {
  fail("EC11 Type recovery Type heartbeat latency must not drift above 20000 ms");
}
if (ec11Recovery.measured_human_disconnect_to_type_heartbeat_ready_ms > 20000) {
  fail("EC11 human-run disconnect-to-heartbeat latency must not drift above 20000 ms");
}
if (ec11Recovery.measured_human_recovery_adv_to_notify_ready_ms > 20000) {
  fail("EC11 human-run recovery advertisement to notify-ready latency must not drift above 20000 ms");
}
for (const token of [
  "type_observed_recovery_address_skips_duplicate_pairing_advertisement_scan",
  "embedded_ble_type_observed_recovery_uses_after_cache_type_pairing_path",
  "type_recovery_promotes_only_trusted_cached_addresses",
  "embedded_ble_notify_advertisement_evidence_skips_only_the_duplicate_scan",
  "ble_name_refresh_and_one_click_use_type_controlled_pairasync_recovery",
  "Program Files\\Listener Type\\listener-type.exe",
  "check-ec11-type-recovery-randomized.ps1",
  "-Iterations 5",
  "-MaxTriggerToTypeReadyMs 10000",
  "<=10000 ms",
  "firmware pre-reset notice",
  "Type pre-reset notice",
  "not a physical GPIO edge measurement",
  "Type-observed direct PairAsync",
  "no duplicate advertisement scan",
  "no paired link check",
  "single-click start/stop without double-click",
]) {
  if (!ec11Recovery.validation_command?.includes(token)) {
    fail(`EC11 Type recovery validation command must preserve token: ${token}`);
  }
}

for (const scriptName of [
  "check-ec11-type-recovery-speed.ps1",
  "check-ec11-type-recovery-randomized.ps1",
]) {
  const ec11SpeedGate = readFileSync(join(repoRoot, "scripts", scriptName), "utf8");
  for (const token of [
    "MaxTriggerToTypeReadyMs = 10000",
    "firmware_generated_ec11_double_after_debounce",
    "physical_gpio_measurement = $false",
  ]) {
    if (!ec11SpeedGate.includes(token)) {
      fail("EC11 Type recovery speed gate " + scriptName + " must keep token: " + token);
    }
  }
}
const ec11SingleSampleGate = readFileSync(
  join(repoRoot, "scripts", "check-ec11-type-recovery-speed.ps1"),
  "utf8",
);
for (const token of [
  "type recovery notice sent before EC11 pairing reset",
  "received EC11 hardware recovery notice before pairing reset",
  "TYPE:READY",
  "The installed Program Files Listener Type process is not running",
]) {
  if (!ec11SingleSampleGate.includes(token)) {
    fail("EC11 Type recovery single-sample gate must keep token: " + token);
  }
}

const startupReconnectGate = readFileSync(
  join(repoRoot, "scripts", "check-type-startup-reconnect-speed.ps1"),
  "utf8",
);
for (const token of [
  "RequirePersistedPath",
  "selected persisted startup audio notify",
  "background listener notify ready",
  "PairAsync appeared during Type restart validation",
  "MaxNotifyReadyMs",
  "TryParse",
  "$null -eq $timestamp",
]) {
  if (!startupReconnectGate.includes(token)) {
    fail(`Type startup reconnect speed gate must keep token: ${token}`);
  }
}

const recordingGate = readFileSync(
  join(repoRoot, "scripts", "check-recording-low-latency-regression.mjs"),
  "utf8",
);
for (const token of [
  "check:asr-latency",
  "check:embedded-ble-processing-led",
  "Active capture low-latency control channel",
]) {
  if (!recordingGate.includes(token)) {
    fail(`recording latency gate must keep token: ${token}`);
  }
}
const asrLatencyGate = readFileSync(
  join(repoRoot, "scripts", "check-volcengine-low-latency-preview.mjs"),
  "utf8",
);
for (const token of [
  "const FINAL_RESULT_TIMEOUT: Duration = Duration::from_secs(12);",
  "final transcript coverage incomplete after full provider timeout",
  "if authoritative_two_pass && current_key != candidate_key",
  "EMBEDDED_AUDIO_TRIM_PAD_SILENCE_MS",
  "LOW_LATENCY_PREVIEW_ENDPOINT",
  "VolcenginePreviewSidecar",
]) {
  if (!asrLatencyGate.includes(token)) {
    fail(`ASR latency gate must keep token: ${token}`);
  }
}

const embeddedBle = readFileSync(join(repoRoot, "src-tauri", "src", "embedded_ble.rs"), "utf8");
const commandsRs = readFileSync(join(repoRoot, "src-tauri", "src", "commands.rs"), "utf8");
const listenerOtaWindow = embeddedBle.match(
  /const\s+LISTENER_OTA_V1_DEFAULT_WINDOW_CHUNKS:\s*usize\s*=\s*(\d+);/,
);
if (!listenerOtaWindow) {
  fail("Listener OTA v1 default window constant is missing");
}
if (Number(listenerOtaWindow[1]) < 100) {
  fail("Listener OTA v1 default window must stay at 100 chunks for the measured <=60000 ms OTA UI target");
}
if (!commandsRs.includes("listener_ota_v1_gatt_probe_snapshot(")) {
  fail("Listener OTA v1 completion must use the fast service-only probe instead of optional DIS metadata");
}
if (!embeddedBle.includes("LISTENER_OTA_V1_WINDOW_ENV")) {
  fail("Listener OTA v1 must keep the window override env for controlled bench experiments");
}
if (
  !embeddedBle.includes("LISTENER_OTA_V1_HANDOFF_DISCOVERY_RETRY_DELAYS")
  || !embeddedBle.includes("reconnect handoff")
  || !embeddedBle.includes("open_listener_ota_v1_target_after_active_link_handoff")
  || embeddedBle.includes("continuing with OTA begin fallback")
) {
  fail("Listener OTA v1 must complete the low-power reconnect handoff before opening the OTA GATT data path");
}
for (const token of [
  "BLE_OTA_OPERATION_MUTEX_NAME",
  "acquire_ble_ota_process_mutex(\"listener_ota_v1\")",
  "BLE OTA operation active in another process; deferring background listener before notify open",
  "BLE OTA operation active in another process; closing idle background listener",
  "BACKGROUND_LISTENER_DEFERRED_FOR_OTA",
  "EMBEDDED_BLE_RETRY_OTA_DEFER_DELAY",
  "TYPE:OTA",
  "Listener OTA v1 reconnect handoff accepted",
  "background listener deferred while firmware OTA is active",
]) {
  if (!embeddedBle.includes(token)) {
    const coordinatorSource = readFileSync(join(repoRoot, "src-tauri", "src", "coordinator.rs"), "utf8");
    const haystack = `${embeddedBle}\n${coordinatorSource}`;
    if (!haystack.includes(token)) {
      fail(`Listener OTA v1 speed guard must keep cross-process BLE exclusivity token: ${token}`);
    }
  }
}

const ota = contracts.ota_transfer_speed;
if (![true, false].includes(ota.accepted)) {
  fail("OTA speed baseline must keep an explicit accepted boolean");
}
for (const key of [
  "target_transfer_bytes_per_ms",
  "acceptance_min_transfer_bytes_per_ms",
  "acceptance_max_post_transfer_recovery_ms",
  "reference_transfer_bytes",
  "reference_target_transfer_ms",
  "reference_acceptance_max_transfer_ms",
  "min_transfer_bytes",
]) {
  requireNumber(ota[key], `OTA speed ${key}`);
}
if (ota.target_transfer_bytes_per_ms !== 20) {
  fail("OTA speed contract must retain the 20 bytes/ms package-sized transfer target");
}
if (
  ota.acceptance_min_transfer_bytes_per_ms !== 18 ||
  ota.acceptance_max_post_transfer_recovery_ms !== 4000
) {
  fail("OTA speed contract must retain the package-sized transfer and <=4 s post-transfer recovery acceptance model");
}
if (
  ota.reference_target_transfer_ms !==
    Math.ceil(ota.reference_transfer_bytes / ota.target_transfer_bytes_per_ms) ||
  ota.reference_acceptance_max_transfer_ms !==
    Math.ceil(ota.reference_transfer_bytes / ota.acceptance_min_transfer_bytes_per_ms)
) {
  fail("OTA speed reference transfer budget must be derived from the transfer package size");
}
if (ota.min_transfer_bytes < 900000) {
  fail("OTA speed contract must validate a real firmware-sized transfer");
}
for (const token of [
  "check-ota-transfer-speed-log.mjs",
  "check-ota-transfer-speed-log.test.mjs",
  "--target-transfer-bytes-per-ms 20",
  "--acceptance-min-transfer-bytes-per-ms 18",
  "--acceptance-max-post-transfer-recovery-ms 4000",
  "--min-bytes 900000",
]) {
  const haystack = `${ota.validation_command ?? ""}\n${packageJson.scripts?.["check:ota-speed-log-contract"] ?? ""}`;
  if (!haystack.includes(token)) {
    fail(`OTA speed validation contract must preserve token: ${token}`);
  }
}
if (
  ota.accepted === false &&
  ![
    "pending_human_acceptance_after_fix",
    "pending_low_power_revalidation_after_cross_process_ota_lock",
    "pending_end_to_end_type_ready_revalidation",
  ].includes(ota.status)
) {
  fail("OTA speed baseline may stay unaccepted only while the focused OTA speed fix or its low-power revalidation is pending");
}
if (ota.accepted === true) {
  for (const key of [
    "measured_bytes",
    "measured_transfer_ms",
    "measured_confirm_ms",
    "measured_type_ready_ms",
  ]) {
    requireNumber(ota[key], `OTA speed ${key}`);
  }
  const acceptanceMaxTransferMs = Math.ceil(
    ota.measured_bytes / ota.acceptance_min_transfer_bytes_per_ms,
  );
  if (ota.measured_transfer_ms > acceptanceMaxTransferMs) {
    fail("accepted OTA speed evidence exceeds its package-sized transfer stage budget");
  }
  if (
    ota.measured_confirm_ms + ota.measured_type_ready_ms >
    ota.acceptance_max_post_transfer_recovery_ms
  ) {
    fail("accepted OTA speed evidence exceeds its combined post-transfer recovery budget");
  }
  if (ota.measured_type_ready !== true) {
    fail("accepted OTA speed evidence must include restored Type notify readiness");
  }
  if (!ota.evidence?.includes("ota") || !ota.evidence?.endsWith(".json")) {
    fail("accepted OTA speed baseline must cite a machine-readable OTA speed artifact");
  }
}

console.log(
  "PASS: performance baselines protect accepted settings-write, BLE rename, EC11 Type recovery, recording latency, Type takeover, package-sized OTA transfer, and <=4 s post-transfer recovery.",
);
