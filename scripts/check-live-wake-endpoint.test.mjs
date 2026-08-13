#!/usr/bin/env node
import assert from "node:assert/strict";
import test from "node:test";
import { analyzeLiveWakeLog } from "./check-live-wake-endpoint.mjs";

function sampleLines(index, overrides = {}) {
  const embeddedId = 700 + index;
  const coordinatorId = `00000000-0000-4000-8000-${String(index).padStart(12, "0")}`;
  const second = String(index).padStart(2, "0");
  const timestamp = `2026-08-12T12:00:${second}.000Z`;
  const wakeMs = overrides.wakeMs ?? 900 + index;
  const tailMs = overrides.tailMs ?? 90 + index;
  const doneMs = overrides.doneMs ?? 40 + index;
  const missing = overrides.missing ?? 0;
  const insertion = overrides.insertion ?? "Inserted";
  const ownerLine = overrides.openGate
    ? `${timestamp} [INFO] [speaker-verification] owner not enrolled for phrase=开始录音 — open gate (any speaker may wake), pcm_ms=990`
    : `${timestamp} [INFO] [speaker-verification] compared enrolled_templates=6 candidate_windows=2 speech_ms=990 inference_ms=52 score=0.72`;
  return [
    ownerLine,
    `${timestamp} [INFO] [wake-phrase] live automatic session activated and released embedded_session_id=${embeddedId} phrase=开始录音 wake_to_capsule_request_ms=${wakeMs} phrase_tail_to_capsule_ms=${tailMs}`,
    `${timestamp} [INFO] [timeline] session_id=Some(${coordinatorId}) event=pcm embedded_session_id=${embeddedId}`,
    `${timestamp} [INFO] [asr] stop_to_transcribing_ms=1 session_id=${coordinatorId} reason=target_speaker_inactive_1000ms timeout_ms=1000 body_started=true sentence_pause=true`,
    `${timestamp} [INFO] [coord] stop_to_done_ms=${doneMs} session_id=${coordinatorId} insertion_status=${insertion} polish_failed=false`,
    `${timestamp} [INFO] [timeline] session_id=${coordinatorId} state=Done`,
    `${timestamp} [INFO] [embedded-ble] capture #1: complete session received; keeping notify open for background listener (session_id=Some(${embeddedId}), pcm_bytes=160000, packets=334)`,
    `${timestamp} [INFO] [embedded-ble] background session completed while keeping notify open pcm_bytes=160000 missing_packets=${missing}`,
  ];
}

function fixture(count = 20, overridesByIndex = new Map()) {
  return Array.from({ length: count }, (_, index) => sampleLines(index, overridesByIndex.get(index))).flat().join("\n");
}

test("passes twenty correlated enrolled-owner wake sessions", () => {
  const report = analyzeLiveWakeLog(fixture());
  assert.equal(report.status, "PASS", report.failures.join("\n"));
  assert.equal(report.sampleCount, 20);
  assert.equal(report.enrolledOwnerSamples, 20);
  assert.equal(report.openGateSamples, 0);
  assert.equal(report.aggregate.wakeToCapsuleP95Ms, 918);
});

test("reports a clean short run as incomplete instead of pass", () => {
  const report = analyzeLiveWakeLog(fixture(1));
  assert.equal(report.status, "INCOMPLETE");
  assert.equal(report.failures.length, 0);
});

test("marks open-gate evidence without pretending it is enrolled-owner evidence", () => {
  const report = analyzeLiveWakeLog(fixture(1, new Map([[0, { openGate: true }]])));
  assert.equal(report.openGateSamples, 1);
  assert.equal(report.enrolledOwnerSamples, 0);
});

test("rejects packet loss and a failed insertion", () => {
  const report = analyzeLiveWakeLog(fixture(20, new Map([[3, { missing: 2, insertion: "ClipboardOnly" }]])));
  assert.equal(report.status, "NO_GO");
  assert.match(report.failures.join("\n"), /missing_packets is nonzero/);
  assert.match(report.failures.join("\n"), /final text was not inserted/);
});

test("rejects an accepted wake over the latency ceiling", () => {
  const report = analyzeLiveWakeLog(fixture(20, new Map([[5, { wakeMs: 1_201 }]])));
  assert.equal(report.status, "NO_GO");
  assert.match(report.failures.join("\n"), /wake latency exceeds 1200 ms/);
});

test("applies the stop-to-done requirement as p95 rather than an invented max", () => {
  const report = analyzeLiveWakeLog(fixture(20, new Map([[19, { doneMs: 700 }]])));
  assert.equal(report.status, "PASS", report.failures.join("\n"));
  assert.equal(report.aggregate.stopToDoneP95Ms, 58);
});
