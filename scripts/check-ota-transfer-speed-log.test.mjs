#!/usr/bin/env node
import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const checker = join(scriptDir, "check-ota-transfer-speed-log.mjs");
const tempRoot = mkdtempSync(join(tmpdir(), "listener-ota-speed-log-"));

function transferReport(fields = {}) {
  return {
    status: "PASS",
    mode: "transfer",
    transfer: {
      transport: "listener_ble_ota_v2",
      bytesTransferred: 960368,
      chunksSent: 1921,
      transferElapsedMs: 57578,
      confirmElapsedMs: 4522,
      confirmedVersion: "1.0.2",
      versionConfirmed: true,
      totalElapsedMs: 63662,
      ...fields,
    },
    errors: [],
  };
}

function lineFor(report) {
  return `2026-07-10T14:48:03Z [INFO] firmware_ota_result_json=${JSON.stringify(report)}`;
}

function runCase(name, text, expectOk) {
  const logPath = join(tempRoot, `${name}.log`);
  const outPath = join(tempRoot, `${name}.json`);
  writeFileSync(logPath, text, "utf8");
  const result = spawnSync(
    process.execPath,
    [
      checker,
      "--log",
      logPath,
      "--output-json",
      outPath,
      "--max-transfer-ms",
      "60000",
      "--max-total-ms",
      "90000",
      "--min-bytes",
      "900000",
    ],
    { encoding: "utf8" },
  );
  if (expectOk) {
    assert.equal(result.status, 0, `${name} should pass\n${result.stderr}`);
  } else {
    assert.notEqual(result.status, 0, `${name} should fail`);
  }
  return JSON.parse(readFileSync(outPath, "utf8"));
}

try {
  const legacy = runCase(
    "legacy-pass",
    "[firmware-ota] BLE OTA result transport=listener_ble_ota_v2 bytes=950944 chunks=1902 transfer_ms=59000 confirm_ms=8992 confirm_attempts=1 confirm_matched=true total_ms=70310\n",
    true,
  );
  assert.equal(legacy.status, "PASS");
  assert.equal(legacy.transferMs, 59000);

  const passAfterFail = runCase(
    "pass-after-fail",
    [
      lineFor({ status: "FAIL", mode: "transfer", transfer: null, errors: ["old failure"] }),
      lineFor(transferReport({ transferElapsedMs: 55000, totalElapsedMs: 61000 })),
    ].join("\n"),
    true,
  );
  assert.equal(passAfterFail.status, "PASS");
  assert.equal(passAfterFail.transferMs, 55000);

  const failAfterPass = runCase(
    "fail-after-pass",
    [
      lineFor(transferReport({ transferElapsedMs: 57578, totalElapsedMs: 63662 })),
      lineFor({
        status: "FAIL",
        mode: "transfer",
        transfer: null,
        errors: ["BLE Listener OTA v2 status read timed out after 15000 ms"],
      }),
    ].join("\n"),
    false,
  );
  assert.equal(failAfterPass.status, "FAIL");
  assert.match(failAfterPass.failures.join("\n"), /timed out/);

  const slowLatest = runCase(
    "slow-latest",
    lineFor(transferReport({ transferElapsedMs: 62442, totalElapsedMs: 68897 })),
    false,
  );
  assert.equal(slowLatest.status, "FAIL");
  assert.match(slowLatest.failures.join("\n"), /transfer_ms=62442/);

  console.log("PASS: OTA speed log parser keeps latest-result semantics and rejects slow or failed latest transfers.");
} finally {
  rmSync(tempRoot, { recursive: true, force: true });
}
