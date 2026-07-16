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
      transport: "denzic_ota_v1",
      bytesTransferred: 960368,
      chunksSent: 1921,
      transferElapsedMs: 51000,
      confirmElapsedMs: 3000,
      confirmedVersion: "1.0.2",
      versionConfirmed: true,
      typeReady: true,
      typeReadyElapsedMs: 820,
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
  const historicalSlowConfirmation = runCase(
    "historical-slow-confirmation",
    "[firmware-ota] BLE OTA result transport=denzic_ota_v1 bytes=950944 chunks=1902 transfer_ms=59000 confirm_ms=8992 confirm_attempts=1 confirm_matched=true type_ready=true type_ready_ms=1100 total_ms=70310\n",
    false,
  );
  assert.equal(historicalSlowConfirmation.status, "FAIL");
  assert.match(
    historicalSlowConfirmation.failures.join("\n"),
    /post_transfer_recovery_ms=10092/,
  );

  const passAfterFail = runCase(
    "pass-after-fail",
    [
      lineFor({ status: "FAIL", mode: "transfer", transfer: null, errors: ["old failure"] }),
      lineFor(transferReport({ transferElapsedMs: 52000, totalElapsedMs: 68256 })),
    ].join("\n"),
    true,
  );
  assert.equal(passAfterFail.status, "PASS");
  assert.equal(passAfterFail.transferMs, 52000);
  assert.equal(passAfterFail.typeReady, true);
  assert.equal(passAfterFail.targetTransferMs, 48019);
  assert.equal(passAfterFail.acceptanceMaxTransferMs, 53354);

  const failAfterPass = runCase(
    "fail-after-pass",
    [
      lineFor(transferReport({ transferElapsedMs: 57578, totalElapsedMs: 63662 })),
      lineFor({
        status: "FAIL",
        mode: "transfer",
        transfer: null,
        errors: ["BLE Listener OTA v1 status read timed out after 15000 ms"],
      }),
    ].join("\n"),
    false,
  );
  assert.equal(failAfterPass.status, "FAIL");
  assert.match(failAfterPass.failures.join("\n"), /timed out/);

  const overTransferStageBudget = runCase(
    "over-transfer-stage-budget",
    lineFor(transferReport({ transferElapsedMs: 53355, totalElapsedMs: 70024 })),
    false,
  );
  assert.equal(overTransferStageBudget.status, "FAIL");
  assert.match(overTransferStageBudget.failures.join("\n"), /payload_transfer_ms=53355/);

  const stagedTransferKeepsHandoffVisible = runCase(
    "staged-transfer-keeps-handoff-visible",
    [
      "[embedded-ble] Denzic OTA v1 #2: transferred 974528/974528 bytes in 1950 data writes, 21 status reads, 0 offset recoveries, active_link_confirmed=true, elapsed_ms=53322, data_write_ms=50028, control_write_ms=2150, status_read_ms=741",
      "[firmware-ota] BLE OTA result transport=denzic_ota_v1 bytes=974528 chunks=1950 transfer_ms=54767 confirm_ms=1866 confirm_attempts=1 confirm_matched=true type_ready=true type_ready_ms=1874 total_ms=58728",
    ].join("\n"),
    true,
  );
  assert.equal(stagedTransferKeepsHandoffVisible.payloadTransferMs, 53322);
  assert.equal(stagedTransferKeepsHandoffVisible.handoffMs, 1445);
  assert.equal(stagedTransferKeepsHandoffVisible.transportTrace.statusReads, 21);

  const largerPackageScalesBudget = runCase(
    "larger-package-scales-budget",
    lineFor(
      transferReport({
        bytesTransferred: 1440000,
        chunksSent: 2880,
        transferElapsedMs: 78000,
        totalElapsedMs: 93000,
      }),
    ),
    true,
  );
  assert.equal(largerPackageScalesBudget.targetTransferMs, 72000);
  assert.equal(largerPackageScalesBudget.acceptanceMaxTransferMs, 80000);

  const postTransferRecoveryRegression = runCase(
    "post-transfer-recovery-regression",
    lineFor(transferReport({ confirmElapsedMs: 6001, totalElapsedMs: 65000 })),
    false,
  );
  assert.match(
    postTransferRecoveryRegression.failures.join("\n"),
    /post_transfer_recovery_ms=6821/,
  );

  const typeReadyRecoveryRegression = runCase(
    "type-ready-recovery-regression",
    lineFor(transferReport({ typeReadyElapsedMs: 4001, totalElapsedMs: 65000 })),
    false,
  );
  assert.match(
    typeReadyRecoveryRegression.failures.join("\n"),
    /post_transfer_recovery_ms=7001/,
  );

  const missingTypeReady = runCase(
    "missing-type-ready",
    lineFor(transferReport({ typeReady: false, totalElapsedMs: 65000 })),
    false,
  );
  assert.equal(missingTypeReady.status, "FAIL");
  assert.match(missingTypeReady.failures.join("\n"), /type_ready=false/);

  console.log("PASS: OTA timing parser keeps latest-result semantics and applies package-sized transfer plus a bounded confirmation-and-Type-ready recovery stage.");
} finally {
  rmSync(tempRoot, { recursive: true, force: true });
}
