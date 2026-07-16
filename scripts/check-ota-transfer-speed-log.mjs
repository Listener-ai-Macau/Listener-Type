#!/usr/bin/env node
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { basename, dirname } from "node:path";

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
const targetTransferBytesPerMs = takeNumber("--target-transfer-bytes-per-ms", 20);
const acceptanceMinTransferBytesPerMs = takeNumber(
  "--acceptance-min-transfer-bytes-per-ms",
  18,
);
const acceptanceMaxPostTransferRecoveryMs = takeNumber(
  "--acceptance-max-post-transfer-recovery-ms",
  4000,
);
const acceptanceMaxHandoffMs = takeNumber(
  "--acceptance-max-handoff-ms",
  1500,
);
const minBytes = takeNumber("--min-bytes", 900000);
const outputJson = takeArg("--output-json", null);

if (!logPath) {
  fail("--log requires a value");
}
for (const [name, value] of [
  ["--target-transfer-bytes-per-ms", targetTransferBytesPerMs],
  ["--acceptance-min-transfer-bytes-per-ms", acceptanceMinTransferBytesPerMs],
  [
    "--acceptance-max-post-transfer-recovery-ms",
    acceptanceMaxPostTransferRecoveryMs,
  ],
]) {
  if (value <= 0) fail(`${name} must be greater than zero`);
}
if (outputJson) {
  mkdirSync(dirname(outputJson), { recursive: true });
}

const text = readFileSync(logPath, "utf8");
const pattern =
  /BLE OTA result transport=(?<transport>\S+) bytes=(?<bytes>\d+) chunks=(?<chunks>\d+) transfer_ms=(?<transferMs>\d+) confirm_ms=(?<confirmMs>\d+) confirm_attempts=(?<confirmAttempts>\d+) confirm_matched=(?<confirmMatched>true|false) type_ready=(?<typeReady>true|false) type_ready_ms=(?<typeReadyMs>\d+) total_ms=(?<totalMs>\d+)/g;
const transportPattern =
  /Denzic OTA v1 #\d+: transferred (?<bytes>\d+)\/(?<totalBytes>\d+) bytes in (?<dataWrites>\d+) data writes, (?<statusReads>\d+) status reads, (?<offsetRecoveries>\d+) offset recoveries, active_link_confirmed=(?<activeLinkConfirmed>true|false), elapsed_ms=(?<protocolMs>\d+)/g;

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
  typeReady: match.groups.typeReady === "true",
  typeReadyMs: Number(match.groups.typeReadyMs),
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
      typeReady: report.transfer.typeReady === true,
      typeReadyMs: Number(report.transfer.typeReadyElapsedMs),
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
const transportResults = [...text.matchAll(transportPattern)].map(match => ({
  index: match.index ?? 0,
  bytes: Number(match.groups.bytes),
  totalBytes: Number(match.groups.totalBytes),
  dataWrites: Number(match.groups.dataWrites),
  statusReads: Number(match.groups.statusReads),
  offsetRecoveries: Number(match.groups.offsetRecoveries),
  activeLinkConfirmed: match.groups.activeLinkConfirmed === "true",
  protocolMs: Number(match.groups.protocolMs),
}));
const latestTransport = transportResults
  .filter(candidate => candidate.index < latest.index && candidate.bytes === latest.bytes)
  .pop() ?? null;
const targetTransferMs = Math.ceil(latest.bytes / targetTransferBytesPerMs);
const acceptanceMaxTransferMs = Math.ceil(
  latest.bytes / acceptanceMinTransferBytesPerMs,
);
const postTransferRecoveryMs = latest.confirmMs + latest.typeReadyMs;
const payloadTransferMs = latestTransport?.protocolMs ?? latest.transferMs;
const handoffMs = latestTransport
  ? Math.max(0, latest.transferMs - latestTransport.protocolMs)
  : null;
const result = {
  status: "PASS",
  log: logPath,
  logName: basename(logPath),
  transport: latest.transport,
  bytes: latest.bytes,
  chunks: latest.chunks,
  transferMs: latest.transferMs,
  payloadTransferMs,
  handoffMs,
  transportTrace: latestTransport,
  confirmMs: latest.confirmMs,
  confirmAttempts: latest.confirmAttempts,
  confirmMatched: latest.confirmMatched,
  typeReady: latest.typeReady,
  typeReadyMs: latest.typeReadyMs,
  targetTransferBytesPerMs,
  acceptanceMinTransferBytesPerMs,
  targetTransferMs,
  acceptanceMaxTransferMs,
  postTransferRecoveryMs,
  acceptanceMaxPostTransferRecoveryMs,
  minBytes,
  observedTransferBytesPerMs:
    payloadTransferMs > 0 ? latest.bytes / payloadTransferMs : null,
};

const failures = [];
if (result.transport !== "denzic_ota_v1") {
  failures.push(`transport=${result.transport}, expected denzic_ota_v1`);
}
if (result.bytes < minBytes) {
  failures.push(`bytes=${result.bytes}, expected >=${minBytes}`);
}
if (payloadTransferMs > acceptanceMaxTransferMs) {
  failures.push(
    `payload_transfer_ms=${payloadTransferMs}, expected <=${acceptanceMaxTransferMs} for ${result.bytes} bytes at ${acceptanceMinTransferBytesPerMs} bytes/ms`,
  );
}
if (handoffMs !== null && handoffMs > acceptanceMaxHandoffMs) {
  failures.push(
    `handoff_ms=${handoffMs}, expected <=${acceptanceMaxHandoffMs} before the payload transfer begins`,
  );
}
if (latestTransport) {
  const requiredStatusReads = Math.floor((latest.chunks - 1) / 100) + 2;
  if (!latestTransport.activeLinkConfirmed) {
    failures.push("active_link_confirmed=false");
  }
  if (latestTransport.statusReads < requiredStatusReads) {
    failures.push(
      `status_reads=${latestTransport.statusReads}, expected >=${requiredStatusReads} for the required 100-packet progress cadence`,
    );
  }
}
if (postTransferRecoveryMs > acceptanceMaxPostTransferRecoveryMs) {
  failures.push(
    `post_transfer_recovery_ms=${postTransferRecoveryMs}, expected <=${acceptanceMaxPostTransferRecoveryMs} (confirmation ${result.confirmMs} ms + Type ready ${result.typeReadyMs} ms)`,
  );
}
if (!result.confirmMatched) {
  failures.push("confirm_matched=false");
}
if (!result.typeReady) {
  failures.push("type_ready=false");
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
  `PASS: Listener OTA stages: payload ${result.bytes} bytes in ${payloadTransferMs} ms (target ${result.targetTransferMs} ms, acceptance <=${result.acceptanceMaxTransferMs} ms); handoff ${handoffMs ?? "legacy-unavailable"} ms; version confirmation ${result.confirmMs} ms; Type ready ${result.typeReadyMs} ms; post-transfer recovery ${postTransferRecoveryMs} ms (acceptance <=${result.acceptanceMaxPostTransferRecoveryMs} ms).`,
);
