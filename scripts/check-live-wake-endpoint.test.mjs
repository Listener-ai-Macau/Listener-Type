#!/usr/bin/env node
import assert from "node:assert/strict";
import test from "node:test";
import { analyzeLiveWakeLog } from "./check-live-wake-endpoint.mjs";

function sampleLines(index, overrides = {}) {
  const embeddedId = 700 + index;
  const coordinatorId = `00000000-0000-4000-8000-${String(index).padStart(12, "0")}`;
  const second = String(index).padStart(2, "0");
  const timestamp = `2026-08-12T12:00:${second}.000Z`;
  const settledTimestamp = `2026-08-12T12:00:${second}.100Z`;
  const endpointLatencyMs = overrides.settledToEndpointMs ?? 1_000;
  const endpointTimestamp = new Date(Date.parse(settledTimestamp) + endpointLatencyMs).toISOString();
  const wakeMs = overrides.wakeMs ?? 900 + index;
  const tailMs = overrides.tailMs ?? 90 + index;
  const doneMs = overrides.doneMs ?? 40 + index;
  const previewMs = overrides.previewMs ?? 1_000 + index;
  const missing = overrides.missing ?? 0;
  const insertion = overrides.insertion ?? "Inserted";
  const providerFinalChars = overrides.providerFinalChars ?? 36;
  const resultFinalChars = overrides.resultFinalChars ?? providerFinalChars;
  const ownerLine = overrides.openGate
    ? `${timestamp} [INFO] [speaker-verification] owner not enrolled for phrase=开始录音 — open gate (any speaker may wake), pcm_ms=990`
    : `${timestamp} [INFO] [speaker-verification] compared enrolled_templates=6 candidate_windows=2 speech_ms=990 inference_ms=52 score=0.72`;
  const lines = [
    ownerLine,
    `${timestamp} [INFO] [wake-phrase] live automatic session activated and released embedded_session_id=${embeddedId} phrase=开始录音 wake_to_capsule_request_ms=${wakeMs} phrase_tail_to_capsule_ms=${tailMs}`,
    `${timestamp} [INFO] [timeline] session_id=Some(${coordinatorId}) ble_start embedded_session_id=${embeddedId}`,
    `${timestamp} [INFO] [obs-v1] {"event":"embedded_audio_preview_first_provider_stream","timing_metric":"preview_latency_ms","timing_value_ms":${previewMs}}`,
    `${settledTimestamp} [INFO] [asr] target-speaker state speaker_id=Some("1") stable_end_ms=Some(4082) audio_duration_ms=Some(4700) provider_audio_duration_ms=Some(4600) pending_provisional=false target_advanced=true pending_advanced=false`,
  ];
  if (overrides.ownerSafeRestore) {
    lines.push(`${timestamp} [INFO] [asr] protocol final restores owner-safe provider text after diarization regression target_chars=${overrides.restoreTargetChars ?? 5} provider_chars=${overrides.restoreProviderChars ?? providerFinalChars}`);
  }
  if (overrides.repeatedPreviewInflation) {
    lines.push(
      `${timestamp} [INFO] [timeline] event=asr_partial session_id=Some(${coordinatorId}) chars=46`,
      `${timestamp} [INFO] [asr] authoritative_bidirectional server metadata: {"has_final_frame":false,"provider_result_chars":49,"result_chars":0}`,
      `${timestamp} [INFO] [timeline] event=asr_partial session_id=Some(${coordinatorId}) chars=64`,
      `${timestamp} [INFO] [asr] authoritative_bidirectional server metadata: {"has_final_frame":false,"provider_result_chars":50,"result_chars":0}`,
      `${timestamp} [INFO] [timeline] event=asr_partial session_id=Some(${coordinatorId}) chars=86`,
      `${timestamp} [INFO] [asr] authoritative_bidirectional server metadata: {"has_final_frame":false,"provider_result_chars":54,"result_chars":0}`,
    );
  }
  lines.push(
    `${timestamp} [INFO] [asr] authoritative_bidirectional server metadata: {"has_final_frame":true,"provider_result_chars":${providerFinalChars},"result_chars":${resultFinalChars}}`,
  );
  if (overrides.firmwareAutomatic) {
    lines.push(`${endpointTimestamp} [INFO] [timeline] event=stop embedded_session_id=${embeddedId} expected_packets=334 origin=VoiceActivation`);
  } else {
    lines.push(`${endpointTimestamp} [INFO] [asr] stop_to_transcribing_ms=1 session_id=${coordinatorId} reason=target_speaker_inactive_1000ms timeout_ms=1000 body_started=true sentence_pause=${overrides.sentencePause ?? false} semantic_continuation=${overrides.semanticContinuation ?? false}`);
  }
  lines.push(
    `${timestamp} [INFO] [coord] stop_to_done_ms=${doneMs} session_id=${coordinatorId} insertion_status=${insertion} polish_failed=false`,
    `${timestamp} [INFO] [timeline] session_id=${coordinatorId} state=Done`,
    `${timestamp} [INFO] [embedded-ble] capture #1: complete session received; keeping notify open for background listener (session_id=Some(${embeddedId}), pcm_bytes=160000, packets=334)`,
    `${timestamp} [INFO] [embedded-ble] background session completed while keeping notify open pcm_bytes=160000 missing_packets=${missing}`,
  );
  return lines;
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

test("ignores continuation lines from a session that began before the cutoff", () => {
  const beforeCutoff = sampleLines(0).slice(0, 3).join("\n");
  const afterCutoff = [
    "2026-08-12T12:01:00.000Z [INFO] [asr] authoritative_bidirectional server metadata: {\"has_final_frame\":false,\"provider_result_chars\":12,\"result_chars\":0}",
    "2026-08-12T12:01:00.100Z [INFO] [timeline] event=asr_partial session_id=Some(00000000-0000-4000-8000-000000000000) chars=12",
  ].join("\n");
  const report = analyzeLiveWakeLog(`${beforeCutoff}\n${afterCutoff}`, {
    after: "2026-08-12T12:00:30.000Z",
  });
  assert.equal(report.status, "INCOMPLETE");
  assert.equal(report.sampleCount, 0);
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
  const report = analyzeLiveWakeLog(fixture(20, new Map([[5, { wakeMs: 1_501 }]])));
  assert.equal(report.status, "NO_GO");
  assert.match(report.failures.join("\n"), /wake latency exceeds 1500 ms/);
});

test("keeps the accepted 1456 ms real-capsule baseline below the hard ceiling", () => {
  const report = analyzeLiveWakeLog(fixture(1, new Map([[0, { wakeMs: 1_456 }]])));
  assert.equal(report.status, "INCOMPLETE", report.failures.join("\n"));
  assert.equal(report.failures.length, 0);
});

test("preserves the owner-accepted installed session 2732 experience baseline", () => {
  const report = analyzeLiveWakeLog(fixture(1, new Map([[0, {
    wakeMs: 773,
    previewMs: 857,
    doneMs: 167,
    firmwareAutomatic: true,
  }]])));
  assert.equal(report.status, "INCOMPLETE", report.failures.join("\n"));
  assert.equal(report.failures.length, 0);
  assert.equal(report.samples[0].wakeToCapsuleMs, 773);
  assert.equal(report.samples[0].previewLatencyMs, 857);
  assert.equal(report.samples[0].stopToDoneMs, 167);
  assert.equal(report.samples[0].missingPackets, 0);
  assert.equal(report.samples[0].insertionStatus, "Inserted");
});

test("accepts firmware VoiceActivation stop as an automatic endpoint", () => {
  const report = analyzeLiveWakeLog(fixture(1, new Map([[0, { firmwareAutomatic: true }]])));
  assert.equal(report.status, "INCOMPLETE", report.failures.join("\n"));
  assert.equal(report.samples[0].firmwareStopOrigin, "VoiceActivation");
  assert.equal(report.samples[0].settledTargetToEndpointMs, 1_000);
});

test("rejects a firmware fallback that leaves settled text open for several seconds", () => {
  const delayed = fixture(1, new Map([[0, {
    firmwareAutomatic: true,
    settledToEndpointMs: 3_878,
  }]]));
  const postStopFinal = "2026-08-12T12:00:05.000Z [INFO] [asr] target-speaker state speaker_id=Some(\"1\") stable_end_ms=Some(4200) pending_provisional=false target_advanced=true";
  const report = analyzeLiveWakeLog(`${delayed}\n${postStopFinal}`);
  assert.equal(report.status, "NO_GO");
  assert.match(report.failures.join("\n"), /settled target endpoint exceeds 1100 ms/);
});

test("does not require sentence punctuation, but rejects a semantic continuation cut", () => {
  const clean = analyzeLiveWakeLog(fixture(1, new Map([[0, { sentencePause: false }]])));
  assert.equal(clean.failures.length, 0, clean.failures.join("\n"));
  const cut = analyzeLiveWakeLog(fixture(1, new Map([[0, { semanticContinuation: true }]])));
  assert.match(cut.failures.join("\n"), /cut a semantic continuation/);
});

test("rejects a slow first preview independently of capsule timing", () => {
  const report = analyzeLiveWakeLog(fixture(20, new Map([[5, { previewMs: 1_801 }]])));
  assert.equal(report.status, "NO_GO");
  assert.match(report.failures.join("\n"), /first preview latency exceeds 1800 ms/);
});

test("rejects owner-safe provider tail restoration that shrinks again at final", () => {
  const report = analyzeLiveWakeLog(fixture(20, new Map([[5, {
    ownerSafeRestore: true,
    restoreProviderChars: 36,
    providerFinalChars: 36,
    resultFinalChars: 5,
  }]])));
  assert.equal(report.status, "NO_GO");
  assert.match(report.failures.join("\n"), /owner-safe provider tail was not preserved/);
});

test("rejects repeated preview growth that outruns small provider window revisions", () => {
  const report = analyzeLiveWakeLog(fixture(20, new Map([[5, {
    repeatedPreviewInflation: true,
  }]])));
  assert.equal(report.status, "NO_GO");
  assert.match(report.failures.join("\n"), /preview repeatedly outgrew the provider revision window/);
});

test("applies the stop-to-done requirement as p95 rather than an invented max", () => {
  const report = analyzeLiveWakeLog(fixture(20, new Map([[19, { doneMs: 700 }]])));
  assert.equal(report.status, "PASS", report.failures.join("\n"));
  assert.equal(report.aggregate.stopToDoneP95Ms, 58);
});
