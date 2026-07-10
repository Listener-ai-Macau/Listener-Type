#!/usr/bin/env node
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import process from "node:process";

const root = process.cwd();
const embeddedBlePath = join(root, "src-tauri", "src", "embedded_ble.rs");
const embeddedBle = readFileSync(embeddedBlePath, "utf8");
const coordinatorPath = join(root, "src-tauri", "src", "coordinator.rs");
const coordinator = readFileSync(coordinatorPath, "utf8");
const dictationPath = join(root, "src-tauri", "src", "coordinator", "dictation.rs");
const dictation = readFileSync(dictationPath, "utf8");
const cliPath = join(root, "src-tauri", "src", "cli.rs");
const cli = readFileSync(cliPath, "utf8");
const libPath = join(root, "src-tauri", "src", "lib.rs");
const lib = readFileSync(libPath, "utf8");
const persistencePath = join(root, "src-tauri", "src", "persistence.rs");
const persistence = readFileSync(persistencePath, "utf8");
const embeddedFileSmokePath = join(
  root,
  "tools",
  "embedded_audio_replay",
  "run_embedded_audio_file_smoke.ps1",
);
const embeddedFileSmoke = readFileSync(embeddedFileSmokePath, "utf8");

function fail(message) {
  throw new Error(message);
}

function section(source, startToken, endToken, label) {
  const start = source.indexOf(startToken);
  if (start < 0) fail(`Could not locate ${label} start token: ${startToken}`);
  const end = source.indexOf(endToken, start + startToken.length);
  if (end < 0) fail(`Could not locate ${label} end token: ${endToken}`);
  return source.slice(start, end);
}

function requireIncludes(source, token, label) {
  if (!source.includes(token)) fail(`${label} missing required token: ${token}`);
}

function requireExcludes(source, token, label) {
  if (source.includes(token)) fail(`${label} must not contain token: ${token}`);
}

const captureSignal = section(
  embeddedBle,
  "enum BleCaptureSignal",
  "struct AudioControlRequest",
  "active capture signal definition",
);
requireExcludes(
  captureSignal,
  "AudioControl",
  "Recording stop/control commands must never share the high-volume BLE notification FIFO",
);
requireExcludes(
  embeddedBle,
  "BleCaptureSignal::AudioControl",
  "Recording stop/control commands must stay off the notification FIFO",
);

const captureLoop = section(
  embeddedBle,
  "fn capture_notification_events_until_cancelled_impl",
  "pub(super) fn collector_has_active_recoverable_session",
  "active capture loop",
);
for (const token of [
  "mpsc::channel::<AudioControlRequest>()",
  "ActiveAudioControlRegistration::install(",
  "cleanup.drain_audio_control_requests(&control_rx);",
  "rx.recv_timeout(receive_timeout)",
]) {
  requireIncludes(captureLoop, token, "Active capture low-latency control channel");
}
if (
  captureLoop.indexOf("cleanup.drain_audio_control_requests(&control_rx);") >
  captureLoop.indexOf("rx.recv_timeout(receive_timeout)")
) {
  fail("Active capture must drain audio control requests before waiting for the next BLE notification");
}

const stopHelper = section(
  embeddedBle,
  "fn send_recording_stop_control_command",
  "pub fn send_recording_control_toggle",
  "recording stop helper",
);
for (const token of [
  "bounded_recording_stop_active_timeout",
  "send_audio_control_via_active_capture",
  'send_control_command_via_usb_serial("VREC:STOP"',
]) {
  requireIncludes(stopHelper, token, "Recording stop helper");
}
for (const token of ["BleFreshGattGuard::enter", "open_audio_control_target_with_retry"]) {
  requireExcludes(
    stopHelper,
    token,
    "Recording stop helper must not open late fresh GATT while audio is streaming",
  );
}

const writePolicy = section(
  embeddedBle,
  "fn audio_control_write_policy",
  "fn audio_control_write_options_from_properties",
  "audio control write policy",
);
requireIncludes(writePolicy, 'bytes == b"VREC:STOP\\n"', "Recording stop write policy");
requireIncludes(writePolicy, "AudioControlWritePolicy::LowLatency", "Recording stop write policy");

const controlRequestHandler = section(
  embeddedBle,
  "fn handle_audio_control_request",
  "fn drain_audio_control_requests",
  "active audio control request handler",
);
for (const token of ["queued_ms", "active audio control dispatch label={}"]) {
  requireIncludes(controlRequestHandler, token, "Runtime latency evidence logging");
}

requireIncludes(
  embeddedFileSmoke,
  "--suppress-capsule-window --force-raw-output --submit-embedded-audio-wav-stream",
  "Installed embedded audio replay smoke must forward capsule suppression and force-raw through the single-instance CLI path",
);
requireIncludes(
  embeddedFileSmoke,
  'LISTENER_TYPE_SUPPRESS_CAPSULE_WINDOW"] = "1"',
  "Installed embedded audio replay smoke must suppress the visible capsule so automation cannot cancel the recording",
);
requireIncludes(
  embeddedFileSmoke,
  "source=backend\\.capsule event=emit_request",
  "Installed embedded audio replay smoke must recover transcript text from capsule emit logs when ASR JSON is truncated",
);
for (const token of [
  "$latestAsr =",
  "[regex]::Match($jsonText",
  "if (-not [string]::IsNullOrWhiteSpace($latestAsr))",
  "return $latestAsr",
]) {
  requireIncludes(
    embeddedFileSmoke,
    token,
    "Installed embedded audio replay smoke must prefer ASR transcript text over terminal capsule status messages",
  );
}

for (const token of [
  "!force_raw_output && mode == PolishMode::Raw",
  "super::raw_style_pack_uses_llm(&pack)",
]) {
  requireIncludes(
    dictation,
    token,
    "Force-raw recording smoke must bypass streaming polish so invalid LLM credentials cannot add latency",
  );
}
requireIncludes(
  dictation,
  "raw_mode_without_llm_uses_passthrough_instead_of_streaming_polish",
  "Raw passthrough streaming eligibility regression test",
);
requireIncludes(
  coordinator,
  "embedded_audio_last_capsule_level: Mutex<f32>",
  "Recording capsule partial preview must preserve the latest audio level",
);
requireIncludes(
  dictation,
  "current_embedded_audio_capsule_level(inner)",
  "Recording capsule partial preview must reuse the latest audio level",
);
for (const token of [
  "fn stabilize_embedded_audio_partial_preview",
  "candidate_key.starts_with(&current_key)",
  "current_key.starts_with(&candidate_key)",
  "embedded_audio_partial_preview_ignores_punctuation_only_revision",
  "embedded_audio_partial_preview_extends_without_rewriting_visible_prefix",
  "embedded_audio_partial_preview_does_not_shrink_visible_text",
]) {
  requireIncludes(
    dictation,
    token,
    "Recording capsule partial preview must grow without rewriting visible text",
  );
}
const partialPreviewEmit = section(
  dictation,
  "fn emit_embedded_audio_partial_preview_if_active",
  "fn emit_embedded_audio_pcm_capsule_if_active",
  "embedded audio partial preview capsule emit",
);
requireExcludes(
  partialPreviewEmit,
  "0.0",
  "ASR partial preview must not reset the recording capsule audio level to zero",
);
requireIncludes(
  dictation,
  "remember_embedded_audio_capsule_level(inner, level)",
  "Embedded BLE PCM capsule updates must cache the latest visible level",
);
requireIncludes(
  coordinator,
  'std::env::var("LISTENER_TYPE_SUPPRESS_CAPSULE_WINDOW")',
  "Capsule automation suppression env",
);
requireIncludes(
  coordinator,
  '!= Some("1")',
  "Capsule automation suppression env",
);
requireIncludes(
  cli,
  "pub fn suppress_capsule_window_requested",
  "CLI capsule suppression parser",
);
requireIncludes(
  cli,
  '"--suppress-capsule-window"',
  "CLI capsule suppression parser",
);
requireIncludes(
  cli,
  "pub fn force_raw_output_requested",
  "CLI force-raw parser",
);
requireIncludes(cli, '"--force-raw-output"', "CLI force-raw parser");
requireIncludes(
  lib,
  "suppress_capsule_window: cli::suppress_capsule_window_requested",
  "Single-instance CLI capsule suppression forwarding",
);
requireIncludes(
  lib,
  "ScopedCapsuleSuppression::apply(suppress_capsule_window)",
  "Single-instance CLI capsule suppression forwarding",
);
requireIncludes(
  lib,
  "force_raw_output: cli::force_raw_output_requested",
  "Single-instance CLI force-raw forwarding",
);
requireIncludes(
  lib,
  "ScopedForceRawOutput::apply(force_raw_output)",
  "Single-instance CLI force-raw forwarding",
);
requireIncludes(
  persistence,
  "legacy_asr_provider_marker_migration_preserves_existing_provider",
  "ASR provider preference migration regression test",
);
requireIncludes(
  persistence,
  "prefs.active_asr_provider_default_migrated = true;",
  "ASR provider preference migration marker",
);
requireExcludes(
  section(
    persistence,
    "let active_asr_provider_default_migrated = raw_prefs",
    "if !streaming_default_migrated",
    "ASR provider default marker migration",
  ),
  "prefs.active_asr_provider =",
  "ASR provider marker migration must not overwrite the user's selected ASR provider",
);

function runNpmScript(scriptName) {
  const commandLine = `npm run ${scriptName}`;
  const result =
    process.platform === "win32"
      ? spawnSync(process.env.ComSpec || "cmd.exe", ["/d", "/s", "/c", commandLine], {
          cwd: root,
          stdio: "inherit",
        })
      : spawnSync("sh", ["-lc", commandLine], {
          cwd: root,
          stdio: "inherit",
        });
  if (result.error) {
    fail(result.error.message);
  }
  if (result.status !== 0) {
    process.exit(result.status ?? 1);
  }
}

runNpmScript("check:asr-latency");
runNpmScript("check:embedded-ble-processing-led");

console.log(
  "PASS: recording latency regression gate keeps low-latency ASR preview and prevents recording stop/control from re-entering the BLE notification FIFO.",
);
