#!/usr/bin/env node
import assert from "node:assert/strict";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import test from "node:test";
import { analyzeLiveWakeOnlyLog } from "./check-live-wake-only.mjs";

function stamp(base, offsetMs) {
  return new Date(base + offsetMs).toISOString();
}

function acceptedAttempt(index, overrides = {}) {
  const embeddedId = 700 + index;
  const coordinatorId = `00000000-0000-4000-8000-${String(index).padStart(12, "0")}`;
  const base = Date.parse("2026-08-12T12:00:00.000Z") + index * 10_000;
  const visibleMs = overrides.visibleMs ?? 900;
  const acceptOffsetMs = overrides.acceptOffsetMs ?? 20;
  const wakeEndMs = overrides.wakeEndMs ?? 650;
  const bodyWaitMs = overrides.bodyWaitMs ?? 3_100;
  const missing = overrides.missing ?? 0;
  const lines = [
    `${stamp(base, 0)} [INFO] source=backend.embedded_ble_session_actor event=ble_packet event=start embedded_session_id=${embeddedId} origin=VoiceActivation`,
    `${stamp(base, 2)} [INFO] source=backend.embedded_ble_session_actor event=ble_packet event=start embedded_session_id=${embeddedId} origin=VoiceActivation`,
    `${stamp(base, 4)} [INFO] source=backend.embedded_ble_session_actor event=ble_packet event=start embedded_session_id=${embeddedId} origin=VoiceActivation`,
    `${stamp(base, visibleMs - 40)} [INFO] source=backend.capsule event=emit_request session_id=${coordinatorId} state=Recording visible=true`,
    `${stamp(base, visibleMs - acceptOffsetMs)} [INFO] [wake-phrase] live automatic session activated and released embedded_session_id=${embeddedId} wake_end_s=${(wakeEndMs / 1_000).toFixed(3)}`,
    `${stamp(base, visibleMs)} [INFO] source=frontend.capsule event=event_received state=recording detail={"sessionId":"${coordinatorId}","insertedChars":null}`,
    `${stamp(base, visibleMs + bodyWaitMs - 50)} [INFO] complete session received; keeping notify open (session_id=Some(${embeddedId}), pcm_bytes=150000, packets=360)`,
    `${stamp(base, visibleMs + bodyWaitMs - 40)} [INFO] embedded_session_id=${embeddedId} coordinator_session_id=${coordinatorId}`,
    `${stamp(base, visibleMs + bodyWaitMs)} [INFO] [coord] wake-only body window expired silently session_id=${coordinatorId}`,
    `${stamp(base, visibleMs + bodyWaitMs + 10)} [INFO] event=asr_final session_id=Some(${coordinatorId}) wake_only_expired transcript_empty=true`,
    `${stamp(base, visibleMs + bodyWaitMs + 20)} [INFO] source=frontend.capsule event=event_received state=idle detail={"sessionId":"${coordinatorId}","insertedChars":null}`,
    `${stamp(base, visibleMs + bodyWaitMs + 30)} [INFO] background session completed while keeping notify open pcm_bytes=150000 missing_packets=${missing}`,
  ];
  if (overrides.sideEffect) {
    lines.splice(-2, 0, `${stamp(base, visibleMs + bodyWaitMs + 15)} [WARN] session_id=${coordinatorId} error_code=emptyTranscript insertion_status=Inserted`);
  }
  return lines;
}

function rejectedAttempt(index, overrides = {}) {
  const embeddedId = 700 + index;
  const coordinatorId = `00000000-0000-4000-8000-${String(index).padStart(12, "0")}`;
  const base = Date.parse("2026-08-12T12:00:00.000Z") + index * 10_000;
  const lines = [
    `${stamp(base, 0)} [INFO] event=start embedded_session_id=${embeddedId} origin=VoiceActivation`,
    `${stamp(base, 1)} [INFO] event=start embedded_session_id=${embeddedId} origin=VoiceActivation`,
    `${stamp(base, 900)} [INFO] [wake-phrase] automatic candidate rejected embedded_session_id=${embeddedId} phrase=<redacted>`,
    `${stamp(base, 950)} [INFO] complete session received; (session_id=Some(${embeddedId}), pcm_bytes=90000, packets=220)`,
    `${stamp(base, 960)} [INFO] background session completed while keeping notify open pcm_bytes=90000 missing_packets=0`,
  ];
  if (overrides.visible) {
    lines.push(`${stamp(base, 920)} [INFO] embedded_session_id=${embeddedId} coordinator_session_id=${coordinatorId}`);
    lines.push(`${stamp(base, 930)} [INFO] source=frontend.capsule event=event_received state=recording detail={"sessionId":"${coordinatorId}"}`);
  }
  return lines;
}

function fixture(accepted = 20, rejected = 0, overrideIndex = null, override = {}) {
  const lines = [];
  for (let index = 0; index < accepted; index += 1) {
    lines.push(...acceptedAttempt(index, index === overrideIndex ? override : {}));
  }
  for (let index = accepted; index < accepted + rejected; index += 1) lines.push(...rejectedAttempt(index));
  return lines.join("\n");
}

function attemptIds(count, start = 700) {
  return Array.from({ length: count }, (_, index) => start + index);
}

test("passes twenty correlated wake-only attempts and deduplicates BLE starts", () => {
  const report = analyzeLiveWakeOnlyLog(fixture(), { attemptIds: attemptIds(20) });
  assert.equal(report.status, "PASS", report.failures.join("\n"));
  assert.equal(report.observedAttemptCount, 20);
  assert.equal(report.successfulAttemptCount, 20);
  assert.equal(report.attempts[0].duplicateStartCount, 2);
  assert.equal(report.aggregate.acceptedWakeToVisibleP95Ms, 20);
  assert.equal(report.aggregate.candidateWakeToVisibleP95Ms, 900);
  assert.equal(report.aggregate.phraseTailToVisibleP95Ms, 250);
});

test("reports a clean short run as incomplete rather than pass", () => {
  const report = analyzeLiveWakeOnlyLog(fixture(3), { attemptIds: attemptIds(3) });
  assert.equal(report.status, "INCOMPLETE", report.failures.join("\n"));
  assert.equal(report.observedAttemptCount, 3);
});

test("classifies accepted body dictation as scenario mismatch rather than product failure", () => {
  const coordinatorId = "00000000-0000-4000-8000-000000000000";
  const body = fixture(1)
    .replace(/.*wake-only body window expired silently.*\n/, "")
    .replace(/.*wake_only_expired transcript_empty=true.*\n/, "")
    .replace(
      /source=frontend\.capsule event=event_received state=idle/,
      `session_id=${coordinatorId} insertion_status=Inserted\n2026-08-12T12:00:04.015Z [INFO] source=frontend.capsule event=event_received state=done detail={"sessionId":"${coordinatorId}","insertedChars":8}\n2026-08-12T12:00:04.020Z [INFO] source=frontend.capsule event=event_received state=idle`,
    );
  const report = analyzeLiveWakeOnlyLog(body, { attemptIds: attemptIds(1) });
  assert.equal(report.status, "INCOMPLETE", report.failures.join("\n"));
  assert.equal(report.eligibleWakeOnlyAttemptCount, 0);
  assert.equal(report.scenarioMismatchBodyCount, 1);
  assert.equal(report.attempts[0].classification, "body_present");
});

test("allows one explicitly rejected attempt under the 19 of 20 stability contract", () => {
  const report = analyzeLiveWakeOnlyLog(fixture(19, 1), { attemptIds: attemptIds(20) });
  assert.equal(report.status, "PASS", report.failures.join("\n"));
  assert.equal(report.rejectedAttemptCount, 1);
  assert.equal(report.successfulAttemptCount, 19);
});

test("rejects two rejected attempts and a rejected attempt that displayed Recording", () => {
  const twoRejected = analyzeLiveWakeOnlyLog(fixture(18, 2), { attemptIds: attemptIds(20) });
  assert.equal(twoRejected.status, "NO_GO");
  assert.match(twoRejected.failures.join("\n"), /only 18\/20 eligible attempts succeeded/);

  const visibleRejected = [
    ...Array.from({ length: 19 }, (_, index) => acceptedAttempt(index)).flat(),
    ...rejectedAttempt(19, { visible: true }),
  ].join("\n");
  const visibleReport = analyzeLiveWakeOnlyLog(visibleRejected, { attemptIds: attemptIds(20) });
  assert.equal(visibleReport.status, "NO_GO");
  assert.match(visibleReport.failures.join("\n"), /rejected candidate displayed Recording capsule/);
});

test("rejects a missing wake-only terminal and insertion or emptyTranscript side effects", () => {
  const missingTerminal = fixture().replace(/.*wake_only_expired transcript_empty=true.*\n/, "");
  const terminalReport = analyzeLiveWakeOnlyLog(missingTerminal, { attemptIds: attemptIds(20) });
  assert.equal(terminalReport.status, "NO_GO");
  assert.match(terminalReport.failures.join("\n"), /missing wake_only_expired terminal event/);

  const sideEffectReport = analyzeLiveWakeOnlyLog(fixture(20, 0, 4, { sideEffect: true }), { attemptIds: attemptIds(20) });
  assert.equal(sideEffectReport.status, "NO_GO");
  assert.match(sideEffectReport.failures.join("\n"), /wake-only side effect/);
});

test("rejects packet loss, QueueFull, notify failures and latency breaches", () => {
  const packetReport = analyzeLiveWakeOnlyLog(fixture(20, 0, 2, { missing: 1 }), { attemptIds: attemptIds(20) });
  assert.equal(packetReport.status, "NO_GO");
  assert.match(packetReport.failures.join("\n"), /missing_packets is nonzero/);

  const faultText = `${fixture()}\n2026-08-12T12:04:00.000Z [ERROR] QueueFull notify_failure=1`;
  const faultReport = analyzeLiveWakeOnlyLog(faultText, { attemptIds: attemptIds(20) });
  assert.equal(faultReport.status, "NO_GO");
  assert.equal(faultReport.transportFaultCount, 1);

  const latencyReport = analyzeLiveWakeOnlyLog(fixture(20, 0, 7, { visibleMs: 1_201, acceptOffsetMs: 1_201, wakeEndMs: 650 }), { attemptIds: attemptIds(20) });
  assert.equal(latencyReport.status, "NO_GO");
  assert.match(latencyReport.failures.join("\n"), /wake-to-visible latency exceeds 500 ms/);
  assert.match(latencyReport.failures.join("\n"), /phrase-tail-to-visible latency exceeds 500 ms/);
});

test("applies after and before bounds without counting out-of-window attempts", () => {
  const report = analyzeLiveWakeOnlyLog(fixture(3), {
    after: "2026-08-12T12:00:10.000Z",
    before: "2026-08-12T12:00:20.000Z",
    attemptIds: [701],
  });
  assert.equal(report.observedAttemptCount, 1);
  assert.equal(report.attempts[0].embeddedSessionId, 701);
});

test("does not misclassify ordinary candidates as wake attempts without explicit labels", () => {
  const report = analyzeLiveWakeOnlyLog(fixture(0, 20));
  assert.equal(report.status, "UNLABELED");
  assert.equal(report.observedCandidateCount, 20);
  assert.equal(report.observedAttemptCount, 0);
  assert.equal(report.eligibleWakeOnlyAttemptCount, 0);
  assert.deepEqual(report.attempts, []);
});

test("operator markers preserve a total device miss as an explicit failed attempt", () => {
  const markers = [
    {
      ordinal: 1,
      markedAt: "2026-08-12T12:00:00.000Z",
      associationEndedAt: "2026-08-12T12:00:01.000Z",
      embeddedSessionId: 700,
    },
    {
      ordinal: 2,
      markedAt: "2026-08-12T12:00:10.000Z",
      associationEndedAt: "2026-08-12T12:00:16.000Z",
      embeddedSessionId: null,
    },
  ];
  const report = analyzeLiveWakeOnlyLog(fixture(1), {
    attemptMarkers: markers,
    requiredAttempts: 2,
    requiredSuccesses: 2,
  });
  assert.equal(report.status, "NO_GO");
  assert.equal(report.observedAttemptCount, 2);
  assert.deepEqual(report.labeling.missingCandidateOrdinals, [2]);
  assert.equal(report.attempts[1].classification, "missing_candidate");
  assert.match(report.failures.join("\n"), /attempt_ordinal=2: no BLE voice-activation candidate/);
});

test("operator markers select only their correlated candidates", () => {
  const report = analyzeLiveWakeOnlyLog(fixture(3), {
    attemptMarkers: [
      {
        ordinal: 1,
        markedAt: "2026-08-12T12:00:10.000Z",
        associationEndedAt: "2026-08-12T12:00:11.000Z",
        embeddedSessionId: 701,
      },
    ],
    requiredAttempts: 1,
    requiredSuccesses: 1,
  });
  assert.equal(report.status, "PASS", report.failures.join("\n"));
  assert.equal(report.observedAttemptCount, 1);
  assert.equal(report.attempts[0].attemptOrdinal, 1);
  assert.equal(report.attempts[0].embeddedSessionId, 701);
  assert.equal(report.labeling.unlabeledCandidateCount, 2);
});

test("CLI accepts a PowerShell UTF-8 BOM marker artifact", () => {
  const dir = mkdtempSync(join(tmpdir(), "listener-wake-marker-"));
  try {
    const log = join(dir, "listener-type.log");
    const markers = join(dir, "markers.json");
    writeFileSync(log, fixture(1), "utf8");
    writeFileSync(
      markers,
      `\uFEFF${JSON.stringify({
        schema: "listener.live-wake-attempt-markers.v1",
        attempts: [
          {
            ordinal: 1,
            markedAt: "2026-08-12T12:00:00.000Z",
            associationEndedAt: "2026-08-12T12:00:07.000Z",
            embeddedSessionId: 700,
          },
        ],
      })}`,
      "utf8",
    );
    const result = spawnSync(
      process.execPath,
      [
        new URL("./check-live-wake-only.mjs", import.meta.url).pathname.slice(1),
        "--log", log,
        "--attempt-markers", markers,
        "--required-attempts", "1",
        "--required-successes", "1",
      ],
      { encoding: "utf8" },
    );
    assert.equal(result.status, 0, result.stderr || result.stdout);
    assert.equal(JSON.parse(result.stdout).status, "PASS");
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
