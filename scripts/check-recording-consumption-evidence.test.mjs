#!/usr/bin/env node
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import assert from "node:assert/strict";
import test from "node:test";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const checker = join(scriptDir, "check-recording-consumption-evidence.mjs");

function makeFixture(options = {}) {
  const dir = mkdtempSync(join(tmpdir(), "listener-recording-consumption-"));
  const start = new Date("2026-07-17T15:00:00.000Z");
  const end = new Date("2026-07-17T15:01:00.000Z");
  const typeTime = new Date(options.typeOutsideWindow ? "2026-07-17T14:00:10.000Z" : "2026-07-17T15:00:52.000Z");
  const capsuleTime = new Date(
    options.capsuleOutsideWindow ? "2026-07-17T14:00:53.000Z" : "2026-07-17T15:00:53.000Z",
  );
  const session = 42;
  const pcmMs = options.pcmMs ?? 50000;
  const packets = options.packets ?? (pcmMs >= 45000 ? 3334 : 667);
  const pcmBytes = packets * 480;
  const elapsedMs = pcmMs + 200;
  const prompt = {
    selected: options.promptSelected ?? "已完成",
    text: "",
    spoken_text: "synthetic self-test fixture",
    output_path: "synthetic",
  };
  const serial = [
    "serial_opened port=COM3 baud=115200 dtr=0 rts=0 no_reset=1",
    "capture_only_ms=60000",
    "I (1000) voice_rec_ctrl: EC11 fast Idle recording dispatch: press_to_control_ms=0 target_ms=50 fallback_hid_suppressed=1 state_before=idle state_after=recording pending=0",
    `I (51000) audio_capture: record session capture integrity: session_id=${session} pcm_ms=${pcmMs} capture_backpressure_gap_ms=0 capture_backpressure_pause_frames=0 transport_backpressure_events=0`,
    `I (51200) ble_audio_stream: audio session transport summary: session=${session} reason=stop elapsed_ms=${elapsedMs} expected_packet_count=${packets} notify_sent=${packets + 4} notify_failed=0 notify_retries=11 msys_waits=3 retry_mbuf=10 retry_enomem=1 retry_tx_timeout=0 retry_tx_status=0 retry_other=0 audio_sent=${packets} audio_pcm_bytes=${pcmBytes} audio_bytes_per_s=31878 audio_packets_per_s=66 audio_failed=0 queue_jobs_purged=0 pool_high_water=8 pool_capacity=264 pool_high_water_pct=3 pool_alloc_failed=0 queue_full=0 replay_retained_high_water=48 replay_stored=${packets} replay_replaced=0 replay_removed=0 replay_resent=0 replay_resend_failed=0 replay_skip_current=0 replay_pending=48 last_drop_reason=none last_error=0`,
    "serial_closed",
  ].join("\n");
  const finalLine =
    options.omitFinal === true
      ? ""
      : `${typeTime.toISOString()} [INFO] [obs-v1] {"event":"embedded_audio_final","contract_version":1,"correlation_id":42,"event_sequence":2,"monotonic_ms":52000,"source":"provider","capability":"audio","ble_lifecycle_state":"connected_idle","command_result":"succeeded","error_category":"none","timing_metric":"final_transcription_ms","timing_value_ms":320}\n`;
  const type = [
    `${typeTime.toISOString()} [INFO] [embedded-ble] capture #99: complete session received; keeping notify open for background listener (session_id=Some(${session}), pcm_bytes=${pcmBytes}, packets=${packets})`,
    `${typeTime.toISOString()} [INFO] [embedded-ble] background session completed while keeping notify open pcm_bytes=${pcmBytes} missing_packets=0`,
    finalLine.trimEnd(),
  ]
    .filter(Boolean)
    .join("\n");
  const capsule = JSON.stringify({
    elapsedMs: 52000,
    event: "emit",
    insertedChars: 128,
    message: null,
    seq: 1,
    sessionId: "synthetic",
    showCapsule: true,
    source: "backend.capsule",
    state: "done",
    translation: false,
    ts: capsuleTime.toISOString(),
    visible: true,
  });

  const paths = {
    serial: join(dir, "serial.log"),
    prompt: join(dir, "prompt.json"),
    type: join(dir, "type.log"),
    capsule: join(dir, "capsule.log"),
  };
  writeFileSync(paths.serial, `${serial}\n`, "utf8");
  writeFileSync(paths.prompt, `${JSON.stringify(prompt)}\n`, "utf8");
  writeFileSync(paths.type, `${type}\n`, "utf8");
  writeFileSync(paths.capsule, `${capsule}\n`, "utf8");
  return { dir, paths, start, end };
}

function runChecker(fixture) {
  return spawnSync(
    process.execPath,
    [
      checker,
      "--serial-log",
      fixture.paths.serial,
      "--prompt-json",
      fixture.paths.prompt,
      "--type-log",
      fixture.paths.type,
      "--capsule-log",
      fixture.paths.capsule,
      "--capture-start-iso",
      fixture.start.toISOString(),
      "--capture-end-iso",
      fixture.end.toISOString(),
    ],
    { encoding: "utf8" },
  );
}

function parseReport(result) {
  assert.equal(result.error, undefined);
  return JSON.parse(result.stdout);
}

test("accepts a correlated 45-60 second recording-consumption fixture", () => {
  const result = runChecker(makeFixture());
  const report = parseReport(result);
  assert.equal(result.status, 0, result.stderr || result.stdout);
  assert.equal(report.status, "PASS");
  assert.equal(report.transport.effectiveConsumptionBps >= 32000, true);
  assert.equal(report.type.sawFinal, true);
});

test("rejects a session without an embedded_audio_final event", () => {
  const result = runChecker(makeFixture({ omitFinal: true }));
  const report = parseReport(result);
  assert.equal(result.status, 1);
  assert.equal(report.status, "NO_GO");
  assert.match(report.errors.join("\n"), /embedded_audio_final/);
});

test("rejects short recordings even when packet counters are clean", () => {
  const result = runChecker(makeFixture({ pcmMs: 10000 }));
  const report = parseReport(result);
  assert.equal(result.status, 1);
  assert.match(report.errors.join("\n"), /below 45000 ms/);
});

test("rejects stale Type and capsule logs outside the capture window", () => {
  const result = runChecker(makeFixture({ typeOutsideWindow: true, capsuleOutsideWindow: true }));
  const report = parseReport(result);
  assert.equal(result.status, 1);
  assert.equal(report.type.linesInWindow, 0);
  assert.equal(report.capsule.eventsInWindow, 0);
  assert.match(report.errors.join("\n"), /Type log has no complete session/);
  assert.match(report.errors.join("\n"), /capsule log has no done event/);
});
