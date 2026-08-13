#!/usr/bin/env node
import assert from "node:assert/strict";
import test from "node:test";
import { makeMarkerReport, voiceActivationStarts } from "./capture-live-wake-attempts.mjs";

test("deduplicates repeated BLE start lines and keeps only numeric session identity", () => {
  const text = [
    "2026-08-13T00:00:00Z event=start embedded_session_id=281 origin=VoiceActivation",
    "2026-08-13T00:00:00Z event=start embedded_session_id=281 origin=VoiceActivation",
    "2026-08-13T00:00:01Z event=start embedded_session_id=282 origin=PhysicalButton",
  ].join("\n");
  assert.deepEqual(voiceActivationStarts(text).map(item => item.embeddedSessionId), [281]);
});

test("marker report preserves an explicit missing-candidate attempt", () => {
  const report = makeMarkerReport({
    log: "listener-type.log",
    associationTimeoutMs: 6_000,
    startedAt: "2026-08-13T00:00:00.000Z",
    completedAt: "2026-08-13T00:00:10.000Z",
    attempts: [
      {
        ordinal: 1,
        markedAt: "2026-08-13T00:00:01.000Z",
        associationEndedAt: "2026-08-13T00:00:07.000Z",
        logByteOffset: 1234,
        embeddedSessionId: null,
      },
    ],
  });
  assert.equal(report.schema, "listener.live-wake-attempt-markers.v1");
  assert.equal(report.attempts[0].embeddedSessionId, null);
  assert.equal(report.attempts[0].logByteOffset, 1234);
  assert.equal(report.associationTimeoutMs, 6_000);
});
