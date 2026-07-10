import { readFileSync } from "node:fs";
import { join } from "node:path";

const repoRoot = process.cwd();
const embeddedBlePath = join(repoRoot, "src-tauri", "src", "embedded_ble.rs");
const source = readFileSync(embeddedBlePath, "utf8");
const dictationPath = join(repoRoot, "src-tauri", "src", "coordinator", "dictation.rs");
const dictationSource = readFileSync(dictationPath, "utf8");

const processingStart = source.indexOf("pub fn send_recording_processing_state");
const processingEnd = source.indexOf("pub fn send_ec11_rotation_mode", processingStart);

if (processingStart < 0 || processingEnd < 0) {
  throw new Error("Could not locate embedded BLE processing command section");
}

const processingSection = source.slice(processingStart, processingEnd);

const forbidden = ["TYPE:BYE", "send_type_bye_after_processing", "audio type bye"];
const foundForbidden = forbidden.filter((token) => processingSection.includes(token));
if (foundForbidden.length > 0) {
  throw new Error(
    `Processing LED sync must not send Type BYE on normal done/warn/stop paths; found ${foundForbidden.join(", ")}`,
  );
}

const processingHelperStart = source.indexOf("fn send_processing_hint_control_command");
const processingHelperEnd = source.indexOf(
  "pub fn send_recording_control_toggle",
  processingHelperStart,
);
if (processingHelperStart < 0 || processingHelperEnd < 0) {
  throw new Error("Could not locate processing LED hint helper");
}
const processingHelperSection = source.slice(processingHelperStart, processingHelperEnd);
for (const token of [
  "send_audio_control_via_active_capture",
  "send_control_command_via_usb_serial",
]) {
  if (!processingHelperSection.includes(token)) {
    throw new Error(`Processing LED hints must keep lightweight active/USB path: ${token}`);
  }
}
for (const token of ["BleFreshGattGuard::enter", "open_audio_control_target_with_retry"]) {
  if (processingHelperSection.includes(token)) {
    throw new Error(
      `Processing LED hints must not open late fresh GATT retry path; found ${token}`,
    );
  }
}
for (const token of [
  "VREC:PROCESSING:START",
  "VREC:PROCESSING:STOP",
  "VREC:PROCESSING:DONE",
  "VREC:PROCESSING:WARN",
]) {
  if (!processingSection.includes(token)) {
    throw new Error(`Processing LED sync lost processing hint command: ${token}`);
  }
}

const stopHelperStart = source.indexOf("fn send_recording_stop_control_command");
const stopHelperEnd = source.indexOf("pub fn send_recording_control_toggle", stopHelperStart);
if (stopHelperStart < 0 || stopHelperEnd < 0) {
  throw new Error("Could not locate embedded BLE recording stop helper");
}
const stopHelperSection = source.slice(stopHelperStart, stopHelperEnd);
for (const token of [
  "bounded_recording_stop_active_timeout",
  "send_audio_control_via_active_capture",
  'send_control_command_via_usb_serial("VREC:STOP"',
  "USB serial stop fallback",
]) {
  if (!stopHelperSection.includes(token)) {
    throw new Error(`Recording stop must keep bounded active/USB fallback path: ${token}`);
  }
}
for (const token of ["BleFreshGattGuard::enter", "open_audio_control_target_with_retry"]) {
  if (stopHelperSection.includes(token)) {
    throw new Error(
      `Recording stop must not open late fresh GATT retry path while audio is streaming; found ${token}`,
    );
  }
}

const stopPolicyStart = source.indexOf("fn audio_control_write_policy");
const stopPolicyEnd = source.indexOf("fn audio_control_write_options_from_properties", stopPolicyStart);
if (stopPolicyStart < 0 || stopPolicyEnd < 0) {
  throw new Error("Could not locate audio control write policy");
}
const stopPolicySection = source.slice(stopPolicyStart, stopPolicyEnd);
if (!stopPolicySection.includes('bytes == b"VREC:STOP\\n"')) {
  throw new Error("Recording stop must prefer low-latency no-response writes when available");
}

const captureSignalStart = source.indexOf("enum BleCaptureSignal");
const captureSignalEnd = source.indexOf("struct AudioControlRequest", captureSignalStart);
if (captureSignalStart < 0 || captureSignalEnd < 0) {
  throw new Error("Could not locate active capture signal/control definitions");
}
const captureSignalSection = source.slice(captureSignalStart, captureSignalEnd);
if (captureSignalSection.includes("AudioControl")) {
  throw new Error(
    "Active audio control must not share the high-volume BLE notification FIFO; keep it on the separate control channel.",
  );
}
for (const token of [
  "mpsc::channel::<AudioControlRequest>()",
  "ActiveAudioControlRegistration::install(",
  "control_tx",
  "cleanup.drain_audio_control_requests(&control_rx)",
  "active audio control dispatch label={}",
]) {
  if (!source.includes(token)) {
    throw new Error(`Active capture control lost its low-latency side channel token: ${token}`);
  }
}
if (source.includes("BleCaptureSignal::AudioControl")) {
  throw new Error("Active audio control regressed back into the BLE notification FIFO");
}

if (!source.includes('write_type_heartbeat(b"TYPE:BYE\\n", "Type heartbeat bye")')) {
  throw new Error("Real notify teardown must still send Type BYE");
}

for (const token of [
  "const DEVICE_AI_PROCESSING_MAX_VISIBLE_MS: u64 = 5_000;",
  "fn schedule_device_ai_processing_max_visible_timeout",
  "dictation_processing_max_visible_timeout",
  "send_recording_processing_done(Duration::from_secs(2))",
  "device AI processing LED max-visible timeout completed",
  "cancel_max_visible_timeout",
]) {
  if (!dictationSource.includes(token)) {
    throw new Error(`Processing LED sync lost max-visible watchdog token: ${token}`);
  }
}

const recoveryStart = source.indexOf("fn listener_recovery_pairing_candidates");
const recoveryEnd = source.indexOf("fn push_listener_pairing_candidate_if_matching", recoveryStart);
if (recoveryStart < 0 || recoveryEnd < 0) {
  throw new Error("Could not locate embedded BLE recovery pairing candidate section");
}

const recoverySection = source.slice(recoveryStart, recoveryEnd);
if (!recoverySection.includes("pairing_device_information_from_bluetooth_address_handle")) {
  throw new Error("Recovery pairing must keep the direct Bluetooth address DeviceInformation fallback");
}
if (!recoverySection.includes("scan_listener_pairing_advertisements")) {
  throw new Error("Recovery pairing must scan advertisements before direct address pairing");
}
if (!recoverySection.includes("recovery pairing using")) {
  throw new Error("Recovery pairing must prefer direct advertisement/address candidates when visible");
}
if (!recoverySection.includes("listener_pairing_candidates_from_unpaired_selector")) {
  throw new Error("Recovery pairing must keep Windows unpaired selector fallback after direct address lookup");
}
const selectorFallbackIndex = recoverySection.indexOf("listener_pairing_candidates_from_unpaired_selector");
const directCandidateIndex = recoverySection.indexOf("listener_recovery_direct_pairing_candidates");
if (selectorFallbackIndex < 0 || directCandidateIndex < 0 || directCandidateIndex > selectorFallbackIndex) {
  throw new Error("Recovery pairing must try direct address lookup before Windows unpaired selector fallback");
}

const directHelperStart = source.indexOf("fn listener_recovery_direct_pairing_candidates");
const directHelperEnd = source.indexOf("fn push_listener_pairing_candidate_if_matching", directHelperStart);
if (directHelperStart < 0 || directHelperEnd < 0) {
  throw new Error("Could not locate embedded BLE direct recovery pairing helper section");
}
const directHelperSection = source.slice(directHelperStart, directHelperEnd);
if (!directHelperSection.includes("pairing_device_information_from_bluetooth_address_handle")) {
  throw new Error("Recovery direct pairing helper must use the Bluetooth address DeviceInformation handle");
}

if (
  source.includes("BTHPORT cache contains advertised Listener address") ||
  source.includes("allowing direct GATT fallback while WinRT paired device table refreshes")
) {
  throw new Error(
    "BTHPORT cache entries are stale-host diagnostics only; they must not authorize direct audio GATT fallback before Windows exposes a paired BLE device.",
  );
}

const advertisementAddressStart = source.indexOf("fn audio_target_advertisement_addresses");
const advertisementAddressEnd = source.indexOf(
  "pub(super) fn remember_current_bluetooth_target_address_for_name",
  advertisementAddressStart,
);
if (advertisementAddressStart < 0 || advertisementAddressEnd < 0) {
  throw new Error("Could not locate audio target advertisement address helper");
}
const advertisementAddressSection = source.slice(advertisementAddressStart, advertisementAddressEnd);
if (!advertisementAddressSection.includes("configured_bluetooth_address_from_env()")) {
  throw new Error("Advertisement address selection must only short-circuit for an explicit env-pinned BLE address");
}
if (advertisementAddressSection.includes("if let Some(address) = configured_bluetooth_address()")) {
  throw new Error("Runtime cached BLE addresses must not short-circuit fresh advertisement/PnP discovery");
}
if (!advertisementAddressSection.includes("listener_pnp_service_signature_addresses()")) {
  throw new Error("Advertisement fallback must include current Windows PnP/service-signature addresses before runtime cache");
}
if (!advertisementAddressSection.includes("runtime_bluetooth_target_address()")) {
  throw new Error("Runtime cached BLE address should remain only as a late fallback candidate");
}

const candidateAllowedStart = source.indexOf("fn ble_candidate_allowed");
const candidateAllowedEnd = source.indexOf("pub(super) fn parse_bluetooth_address_hex", candidateAllowedStart);
if (candidateAllowedStart < 0 || candidateAllowedEnd < 0) {
  throw new Error("Could not locate BLE candidate filter helper");
}
const candidateAllowedSection = source.slice(candidateAllowedStart, candidateAllowedEnd);
if (!candidateAllowedSection.includes("configured_bluetooth_address_from_env()")) {
  throw new Error("BLE candidate filter must still honor explicit env-pinned BLE addresses");
}
if (candidateAllowedSection.includes("configured_bluetooth_address()")) {
  throw new Error("BLE candidate filter must not hard-reject current Windows candidates by runtime cache");
}
if (!candidateAllowedSection.includes("listener_recovery_target_addresses()")) {
  throw new Error("BLE candidate filter must keep Windows PnP/service addresses as a positive trust signal");
}

console.log(
  "PASS: embedded BLE recording stop uses low-latency active/USB paths with a separate control side channel, processing LED sync avoids late fresh GATT, recovery pairing appends direct address fallback, stale BTHPORT cache does not bypass pairing, and runtime BLE address cache cannot outrank current Windows evidence.",
);
