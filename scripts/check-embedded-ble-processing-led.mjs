import { readFileSync } from "node:fs";
import { join } from "node:path";

const repoRoot = process.cwd();
const embeddedBlePath = join(repoRoot, "src-tauri", "src", "embedded_ble.rs");
const source = readFileSync(embeddedBlePath, "utf8");

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

for (const token of [
  "send_type_ready_keepalive_before_processing",
  "audio type ready before processing done",
  "audio type ready before processing warning",
]) {
  if (!processingSection.includes(token)) {
    throw new Error(`Processing LED sync lost Type-ready keepalive token: ${token}`);
  }
}

if (!source.includes('write_type_heartbeat(b"TYPE:BYE\\n", "Type heartbeat bye")')) {
  throw new Error("Real notify teardown must still send Type BYE");
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

console.log(
  "PASS: embedded BLE processing LED sync keeps Type-ready across normal completion, reserves BYE for teardown, recovery pairing appends direct address fallback, and stale BTHPORT cache does not bypass pairing.",
);
