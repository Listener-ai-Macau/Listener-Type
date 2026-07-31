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
const acceptanceMinTransferBytesPerMs = takeNumber(
  "--acceptance-min-transfer-bytes-per-ms",
  18,
);
const acceptanceMaxNonTransferFixedMs = takeNumber(
  "--acceptance-max-non-transfer-fixed-ms",
  7000,
);
const minBytes = takeNumber("--min-bytes", 900000);
const outputJson = takeArg("--output-json", null);

if (!logPath) {
  fail("--log requires a value");
}
for (const [name, value] of [
  ["--acceptance-min-transfer-bytes-per-ms", acceptanceMinTransferBytesPerMs],
  [
    "--acceptance-max-non-transfer-fixed-ms",
    acceptanceMaxNonTransferFixedMs,
  ],
]) {
  if (value <= 0) fail(`${name} must be greater than zero`);
}
if (outputJson) {
  mkdirSync(dirname(outputJson), { recursive: true });
}

const text = readFileSync(logPath, "utf8");
const pattern =
  /BLE OTA result transport=(?<transport>\S+) bytes=(?<bytes>\d+) chunks=(?<chunks>\d+)(?: pretransfer_type_ready=(?<pretransferTypeReady>true|false) pretransfer_type_ready_ms=(?<pretransferTypeReadyMs>\d+))? transfer_ms=(?<transferMs>\d+) confirm_ms=(?<confirmMs>\d+) confirm_attempts=(?<confirmAttempts>\d+) confirm_matched=(?<confirmMatched>true|false) type_ready=(?<typeReady>true|false) type_ready_ms=(?<typeReadyMs>\d+) total_ms=(?<totalMs>\d+)/g;
const transportPattern =
  /Denzic OTA v1 #\d+: transferred (?<bytes>\d+)\/(?<totalBytes>\d+) bytes in (?<dataWrites>\d+) data writes, (?<statusReads>\d+) status reads, (?<offsetRecoveries>\d+) offset recoveries(?:, resumed_bytes=(?<resumedBytes>\d+))?, active_link_confirmed=(?<activeLinkConfirmed>true|false), elapsed_ms=(?<protocolMs>\d+)(?:, bulk_kb_s=(?<bulkKbS>[\d.]+))?(?:, data_write_ms=(?<dataWriteMs>\d+), control_write_ms=(?<controlWriteMs>\d+), status_read_ms=(?<statusReadMs>\d+)(?:, non_transfer_ms=(?<nonTransferMs>\d+))?)?/g;

const results = [...text.matchAll(pattern)].map((match) => ({
  status: "PASS",
  index: match.index ?? 0,
  transport: match.groups.transport,
  bytes: Number(match.groups.bytes),
  chunks: Number(match.groups.chunks),
  pretransferTypeReady: match.groups.pretransferTypeReady === "true",
  pretransferTypeReadyMs: Number(match.groups.pretransferTypeReadyMs ?? 0),
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
      pretransferTypeReady: report.transfer.pretransferTypeReady === true,
      pretransferTypeReadyMs: Number(
        report.transfer.pretransferTypeReadyElapsedMs ?? 0,
      ),
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
  resumedBytes: Number(match.groups.resumedBytes ?? 0),
  activeLinkConfirmed: match.groups.activeLinkConfirmed === "true",
  protocolMs: Number(match.groups.protocolMs),
  bulkKbS: Number(match.groups.bulkKbS ?? 0),
  dataWriteMs: Number(match.groups.dataWriteMs ?? 0),
  controlWriteMs: Number(match.groups.controlWriteMs ?? 0),
  statusReadMs: Number(match.groups.statusReadMs ?? 0),
  nonTransferMs: Number(match.groups.nonTransferMs ?? 0),
}));
const latestTransport = transportResults
  .filter(candidate => candidate.index < latest.index && candidate.bytes === latest.bytes)
  .pop() ?? null;
const acceptanceMaxTransferMs = Math.ceil(
  latest.bytes / acceptanceMinTransferBytesPerMs,
);
const postTransferRecoveryMs = latest.confirmMs + latest.typeReadyMs;
const payloadTransferMs = latestTransport?.protocolMs ?? latest.transferMs;
const transferOrchestrationMs = latestTransport
  ? Math.max(0, latest.transferMs - latestTransport.protocolMs)
  : null;
const nonTransferFixedElapsedMs =
  latest.totalMs >= payloadTransferMs ? latest.totalMs - payloadTransferMs : null;
const knownFixedStageMs =
  latest.pretransferTypeReadyMs +
  (transferOrchestrationMs ?? 0) +
  latest.confirmMs +
  latest.typeReadyMs;
const packageIntegrityAndCommandBookkeepingMs =
  nonTransferFixedElapsedMs === null
    ? null
    : Math.max(0, nonTransferFixedElapsedMs - knownFixedStageMs);
const protocolInstrumentedMs = latestTransport
  ? latestTransport.dataWriteMs +
    latestTransport.controlWriteMs +
    latestTransport.statusReadMs
  : null;
const protocolSchedulingAndCallbackMs =
  protocolInstrumentedMs === null
    ? null
    : Math.max(0, payloadTransferMs - protocolInstrumentedMs);
const result = {
  status: "PASS",
  log: logPath,
  logName: basename(logPath),
  transport: latest.transport,
  bytes: latest.bytes,
  chunks: latest.chunks,
  pretransferTypeReady: latest.pretransferTypeReady,
  pretransferTypeReadyMs: latest.pretransferTypeReadyMs,
  transferMs: latest.transferMs,
  payloadTransferMs,
  transportTrace: latestTransport,
  confirmMs: latest.confirmMs,
  confirmAttempts: latest.confirmAttempts,
  confirmMatched: latest.confirmMatched,
  typeReady: latest.typeReady,
  typeReadyMs: latest.typeReadyMs,
  acceptanceMinTransferBytesPerMs,
  acceptanceMaxTransferMs,
  transferOrchestrationMs,
  nonTransferFixedElapsedMs,
  acceptanceMaxNonTransferFixedMs,
  packageIntegrityAndCommandBookkeepingMs,
  protocolInstrumentedMs,
  protocolSchedulingAndCallbackMs,
  postTransferRecoveryMs,
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
if (latestTransport) {
  // Production negotiates at most 400 chunks per SYNC window. The transfer
  // reads status after START, each full window, and the final partial window.
  const requiredStatusReads = Math.floor((latest.chunks - 1) / 400) + 2;
  if (!latestTransport.activeLinkConfirmed) {
    failures.push("active_link_confirmed=false");
  }
  if (latestTransport.offsetRecoveries !== 0) {
    failures.push(
      `offset_recoveries=${latestTransport.offsetRecoveries}, expected 0`,
    );
  }
  if (latestTransport.resumedBytes !== 0) {
    failures.push(`resumed_bytes=${latestTransport.resumedBytes}, expected 0`);
  }
  if (latestTransport.statusReads < requiredStatusReads) {
    failures.push(
      `status_reads=${latestTransport.statusReads}, expected >=${requiredStatusReads} for the production 400-chunk SYNC cadence`,
    );
  }
}
if (nonTransferFixedElapsedMs === null) {
  failures.push(
    `total_ms=${latest.totalMs} is earlier than ota_protocol_transfer_ms=${payloadTransferMs}`,
  );
} else if (nonTransferFixedElapsedMs > acceptanceMaxNonTransferFixedMs) {
  failures.push(
    `non_transfer_fixed_elapsed_ms=${nonTransferFixedElapsedMs}, expected <=${acceptanceMaxNonTransferFixedMs} (total ${latest.totalMs} ms - ota_protocol_transfer ${payloadTransferMs} ms)`,
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
  `PASS: Listener OTA stages: protocol ${result.bytes} bytes in ${payloadTransferMs} ms (${result.observedTransferBytesPerMs.toFixed(3)} kB/s, acceptance >=${result.acceptanceMinTransferBytesPerMs} kB/s); non-transfer fixed ${nonTransferFixedElapsedMs} ms (acceptance <=${result.acceptanceMaxNonTransferFixedMs} ms); pre-transfer Type ready ${result.pretransferTypeReadyMs} ms; transfer orchestration ${transferOrchestrationMs ?? "legacy-unavailable"} ms; version confirmation ${result.confirmMs} ms; Type ready ${result.typeReadyMs} ms.`,
);
