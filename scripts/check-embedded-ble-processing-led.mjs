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

console.log("PASS: embedded BLE processing LED sync keeps Type-ready across normal completion and reserves BYE for teardown.");
