#!/usr/bin/env node
import { readFileSync } from "node:fs";
import { join } from "node:path";

const repoRoot = process.cwd();
const baselinePath = join(repoRoot, "scripts", "performance-baselines.json");
const baselines = JSON.parse(readFileSync(baselinePath, "utf8"));

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
if (rename.max_random_rename_elapsed_ms > 25000) {
  fail("BLE random rename latency ceiling must not drift above 25000 ms");
}
if (rename.max_restore_elapsed_ms > 25000) {
  fail("BLE rename restore latency ceiling must not drift above 25000 ms");
}
if (!rename.validation_command?.includes("--rename-random-name")) {
  fail("BLE rename recovery command must run the full random-name rename path");
}
if (!rename.validation_command?.includes("--max-write-ms 25000")) {
  fail("BLE rename recovery command must enforce the random rename latency ceiling");
}
if (!rename.validation_command?.includes("--max-restore-ms 25000")) {
  fail("BLE rename recovery command must enforce the restore latency ceiling");
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
]) {
  requireNumber(ec11Recovery[key], `EC11 Type recovery ${key}`);
}
if (ec11Recovery.max_recovery_trigger_to_notify_ready_ms > 20000) {
  fail("EC11 Type recovery notify-ready latency ceiling must not drift above 20000 ms");
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
  !ec11Recovery.evidence?.includes("ec11-boundary-20260710-20s") ||
  !ec11Recovery.evidence?.endsWith("firmware-ec11-double-observed-address.log")
) {
  fail("EC11 Type recovery baseline must cite the observed-address firmware double-click artifact");
}
if (
  !ec11Recovery.type_log_evidence?.includes("ec11-boundary-20260710-20s") ||
  !ec11Recovery.type_log_evidence?.endsWith("type-log-after-human-rootfix-recovery-slice.txt")
) {
  fail("EC11 Type recovery baseline must cite the human-run Type recovery log slice");
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
  "<=20000 ms",
  "Type-observed direct PairAsync",
  "no duplicate advertisement scan",
  "no paired link check",
  "single-click start/stop without double-click",
]) {
  if (!ec11Recovery.validation_command?.includes(token)) {
    fail(`EC11 Type recovery validation command must preserve token: ${token}`);
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
  "Volcengine preview remains",
  "LOW_LATENCY_PREVIEW_ENDPOINT",
  "VolcenginePreviewSidecar",
]) {
  if (!asrLatencyGate.includes(token)) {
    fail(`ASR latency gate must keep token: ${token}`);
  }
}

const ota = contracts.ota_transfer_speed;
if (ota.accepted !== false || ota.status !== "pending_baseline_after_ota_acceptance") {
  fail("OTA speed baseline must remain explicit pending work until OTA is accepted");
}

console.log(
  "PASS: performance baselines protect accepted settings-write, BLE rename, EC11 Type recovery, recording latency, and Type takeover targets; OTA speed baseline remains explicit pending work.",
);
