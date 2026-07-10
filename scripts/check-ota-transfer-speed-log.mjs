#!/usr/bin/env node
import { readFileSync, writeFileSync } from "node:fs";
import { basename } from "node:path";

function fail(message) {
  throw new Error(message);
}

function takeArg(name, fallback = null) {
  const index = process.argv.indexOf(name);
  if (index === -1) return fallback;
  const value = process.argv[index + 1];
  if (!value || value.startsWith("--")) {
    fail(`${name} requires a value`);
  }
  return value;
}

function takeNumber(name, fallback) {
  const raw = takeArg(name, String(fallback));
  const value = Number(raw);
  if (!Number.isFinite(value)) {
    fail(`${name} must be a finite number`);
  }
  return value;
}

const logPath = takeArg("--log");
const maxTransferMs = takeNumber("--max-transfer-ms", 60000);
const maxTotalMs = takeNumber("--max-total-ms", 90000);
const minBytes = takeNumber("--min-bytes", 900000);
const outputJson = takeArg("--output-json", null);

const text = readFileSync(logPath, "utf8");
const pattern =
  /BLE OTA result transport=(?<transport>\S+) bytes=(?<bytes>\d+) chunks=(?<chunks>\d+) transfer_ms=(?<transferMs>\d+) confirm_ms=(?<confirmMs>\d+) confirm_attempts=(?<confirmAttempts>\d+) confirm_matched=(?<confirmMatched>true|false) total_ms=(?<totalMs>\d+)/g;

const results = [...text.matchAll(pattern)].map((match) => ({
  status: "PASS",
  index: match.index ?? 0,
  transport: match.groups.transport,
  bytes: Number(match.groups.bytes),
  chunks: Number(match.groups.chunks),
  transferMs: Number(match.groups.transferMs),
  confirmMs: Number(match.groups.confirmMs),
  confirmAttempts: Number(match.groups.confirmAttempts),
  confirmMatched: match.groups.confirmMatched === "true",
  totalMs: Number(match.groups.totalMs),
}));

let searchFrom = 0;
for (const line of text.split(/\r?\n/)) {
  const lineIndex = text.indexOf(line, searchFrom);
  searchFrom = lineIndex + line.length + 1;
  const marker = "firmware_ota_result_json=";
  const markerIndex = line.indexOf(marker);
  if (markerIndex === -1) continue;
  try {
    const report = JSON.parse(line.slice(markerIndex + marker.length));
    if (report?.mode !== "transfer") continue;
    if (report?.status === "FAIL" || !report.transfer) {
      results.push({
        status: "FAIL",
        index: lineIndex,
        errors: Array.isArray(report?.errors) ? report.errors : ["OTA transfer failed"],
      });
      continue;
    }
    results.push({
      status: "PASS",
      index: lineIndex,
      transport: report.transfer.transport,
      bytes: Number(report.transfer.bytesTransferred),
      chunks: Number(report.transfer.chunksSent),
      transferMs: Number(report.transfer.transferElapsedMs),
      confirmMs: Number(report.transfer.confirmElapsedMs),
      confirmAttempts: Number(report.transfer.confirmedVersion ? 1 : 0),
      confirmMatched: report.transfer.versionConfirmed === true,
      totalMs: Number(report.transfer.totalElapsedMs),
    });
  } catch {
    // Ignore malformed historical lines; the latest valid transfer below is authoritative.
  }
}

results.sort((a, b) => a.index - b.index);
if (results.length === 0) {
  fail(`No BLE OTA result line found in ${logPath}`);
}

const latest = results[results.length - 1];
if (latest.status === "FAIL") {
  const result = {
    status: "FAIL",
    log: logPath,
    logName: basename(logPath),
    failures: latest.errors,
  };
  if (outputJson) {
    writeFileSync(outputJson, `${JSON.stringify(result, null, 2)}\n`);
  }
  fail(`Latest BLE OTA transfer failed: ${(latest.errors ?? []).join("; ")}`);
}
const result = {
  status: "PASS",
  log: logPath,
  logName: basename(logPath),
  transport: latest.transport,
  bytes: latest.bytes,
  chunks: latest.chunks,
  transferMs: latest.transferMs,
  confirmMs: latest.confirmMs,
  confirmAttempts: latest.confirmAttempts,
  confirmMatched: latest.confirmMatched,
  totalMs: latest.totalMs,
  maxTransferMs,
  maxTotalMs,
  minBytes,
};

const failures = [];
if (result.transport !== "listener_ble_ota_v2") {
  failures.push(`transport=${result.transport}, expected listener_ble_ota_v2`);
}
if (result.bytes < minBytes) {
  failures.push(`bytes=${result.bytes}, expected >=${minBytes}`);
}
if (result.transferMs > maxTransferMs) {
  failures.push(`transfer_ms=${result.transferMs}, expected <=${maxTransferMs}`);
}
if (result.totalMs > maxTotalMs) {
  failures.push(`total_ms=${result.totalMs}, expected <=${maxTotalMs}`);
}
if (!result.confirmMatched) {
  failures.push("confirm_matched=false");
}

if (failures.length > 0) {
  result.status = "FAIL";
  result.failures = failures;
}

if (outputJson) {
  writeFileSync(outputJson, `${JSON.stringify(result, null, 2)}\n`, "utf8");
}

if (failures.length > 0) {
  fail(`OTA speed regression: ${failures.join("; ")}`);
}

console.log(
  `PASS: Listener OTA v2 transfer ${result.bytes} bytes in ${result.transferMs} ms, total ${result.totalMs} ms`,
);
