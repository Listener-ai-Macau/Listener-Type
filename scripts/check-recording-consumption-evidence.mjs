#!/usr/bin/env node
import { readFileSync, writeFileSync, mkdirSync, statSync } from "node:fs";
import { dirname } from "node:path";
import process from "node:process";

const args = parseArgs(process.argv.slice(2));

function parseArgs(argv) {
  const parsed = {
    minDurationMs: 45000,
    maxDurationMs: 65000,
    maxPressToControlMs: 50,
    maxPoolHighWaterPct: 20,
    maxRetryRatio: 0.01,
    timeSlackMs: 10000,
  };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (!arg.startsWith("--")) {
      failUsage(`Unexpected argument: ${arg}`);
    }
    const key = arg.slice(2).replace(/-([a-z])/g, (_, ch) => ch.toUpperCase());
    const value = argv[i + 1];
    if (value === undefined || value.startsWith("--")) {
      failUsage(`Missing value for ${arg}`);
    }
    i += 1;
    if (
      [
        "minDurationMs",
        "maxDurationMs",
        "maxPressToControlMs",
        "maxPoolHighWaterPct",
        "timeSlackMs",
      ].includes(key)
    ) {
      parsed[key] = Number.parseInt(value, 10);
    } else if (key === "maxRetryRatio") {
      parsed[key] = Number.parseFloat(value);
    } else {
      parsed[key] = value;
    }
  }
  for (const required of ["serialLog", "promptJson"]) {
    if (!parsed[required]) {
      failUsage(`Missing --${required.replace(/[A-Z]/g, (ch) => `-${ch.toLowerCase()}`)}`);
    }
  }
  return parsed;
}

function failUsage(message) {
  console.error(
    `${message}\n\nUsage: node scripts/check-recording-consumption-evidence.mjs ` +
      "--serial-log <firmware.log> --prompt-json <prompt-result.json> " +
      "[--type-log <listener-type.log>] [--capsule-log <capsule-timeline.log>] " +
      "[--capture-start-iso <iso>] [--capture-end-iso <iso>] " +
      "[--output-json <result.json>]",
  );
  process.exit(2);
}

function readText(path) {
  return readFileSync(path, "utf8");
}

function readJson(path) {
  const text = readText(path).trim();
  if (!text) {
    return null;
  }
  return JSON.parse(text);
}

function parseKv(body) {
  const values = {};
  const re = /([a-zA-Z_][a-zA-Z0-9_]*)=("[^"]*"|\S+)/g;
  for (const match of body.matchAll(re)) {
    let value = match[2];
    if (value.startsWith('"') && value.endsWith('"')) {
      value = value.slice(1, -1);
    }
    values[match[1]] = value;
  }
  return values;
}

function toInt(value) {
  if (value === undefined || value === null || value === "") {
    return null;
  }
  const parsed = Number.parseInt(String(value), 10);
  return Number.isFinite(parsed) ? parsed : null;
}

function espMs(line) {
  const match = /\((\d+)\)/.exec(line);
  return match ? Number.parseInt(match[1], 10) : null;
}

const errors = [];
const warnings = [];
const serialText = readText(args.serialLog);
const serialLines = serialText.split(/\r?\n/);
const prompt = readJson(args.promptJson);
const captureWindow = inferCaptureWindow(args.serialLog, serialText);

if (!prompt) {
  errors.push("prompt result JSON is empty or missing");
} else if (prompt.selected !== "已完成") {
  errors.push(`operator prompt selected ${JSON.stringify(prompt.selected)} instead of "已完成"`);
}

const dispatches = [];
const activeStops = [];
const summaries = [];
const integrityBySession = new Map();

for (const [index, line] of serialLines.entries()) {
  const dispatch = /EC11 fast Idle recording dispatch: press_to_control_ms=(\d+) target_ms=(\d+)/.exec(
    line,
  );
  if (dispatch) {
    dispatches.push({
      line: index + 1,
      espMs: espMs(line),
      pressToControlMs: Number.parseInt(dispatch[1], 10),
      targetMs: Number.parseInt(dispatch[2], 10),
    });
  }
  if (/fast active recording stop queued/.test(line) || /fast active recording stop dispatch/.test(line)) {
    activeStops.push({ line: index + 1, espMs: espMs(line), text: line.trim() });
  }
  const integrity = /record session capture integrity: session_id=(\d+) pcm_ms=(\d+) capture_backpressure_gap_ms=(\d+) capture_backpressure_pause_frames=(\d+) transport_backpressure_events=(\d+)/.exec(
    line,
  );
  if (integrity) {
    integrityBySession.set(Number.parseInt(integrity[1], 10), {
      session: Number.parseInt(integrity[1], 10),
      pcmMs: Number.parseInt(integrity[2], 10),
      captureBackpressureGapMs: Number.parseInt(integrity[3], 10),
      captureBackpressurePauseFrames: Number.parseInt(integrity[4], 10),
      transportBackpressureEvents: Number.parseInt(integrity[5], 10),
      line: index + 1,
      espMs: espMs(line),
    });
  }
  const summary = /audio session transport summary: session=(\d+) (.*)/.exec(line);
  if (summary) {
    const fields = parseKv(summary[2]);
    const session = Number.parseInt(summary[1], 10);
    summaries.push({
      session,
      line: index + 1,
      espMs: espMs(line),
      fields,
      integrity: integrityBySession.get(session) ?? null,
    });
  }
}

if (dispatches.length === 0) {
  errors.push("serial log has no EC11 fast Idle recording dispatch");
}
for (const dispatch of dispatches) {
  if (dispatch.pressToControlMs > args.maxPressToControlMs) {
    errors.push(
      `EC11 fast Idle dispatch press_to_control_ms=${dispatch.pressToControlMs} exceeds ${args.maxPressToControlMs}`,
    );
  }
}
const firstDispatch = dispatches[0] ?? null;
if (firstDispatch?.espMs !== null) {
  for (const stop of activeStops) {
    if (stop.espMs !== null && stop.espMs - firstDispatch.espMs <= 500) {
      errors.push(
        `fast active recording stop appeared ${stop.espMs - firstDispatch.espMs} ms after fast Idle dispatch at line ${stop.line}`,
      );
    }
  }
}

if (summaries.length === 0) {
  errors.push("serial log has no audio session transport summary");
}
const selectedSummary =
  summaries
    .slice()
    .sort((a, b) => (toInt(b.fields.audio_pcm_bytes) ?? 0) - (toInt(a.fields.audio_pcm_bytes) ?? 0))[0] ??
  null;
const transport = selectedSummary ? analyzeTransport(selectedSummary) : null;

function analyzeTransport(summary) {
  const fields = summary.fields;
  const expectedPacketCount = toInt(fields.expected_packet_count);
  const audioSent = toInt(fields.audio_sent);
  const audioPcmBytes = toInt(fields.audio_pcm_bytes);
  const elapsedMs = toInt(fields.elapsed_ms);
  const retryMbuf = toInt(fields.retry_mbuf) ?? 0;
  const retryEnomem = toInt(fields.retry_enomem) ?? 0;
  const poolHighWaterPct = toInt(fields.pool_high_water_pct);
  const audioFailed = toInt(fields.audio_failed) ?? 0;
  const queueFull = toInt(fields.queue_full) ?? 0;
  const poolAllocFailed = toInt(fields.pool_alloc_failed) ?? 0;
  const notifyFailed = toInt(fields.notify_failed) ?? 0;
  const lastDropReason = fields.last_drop_reason ?? null;
  const integrity = summary.integrity;
  const durationMs = integrity?.pcmMs ?? elapsedMs;
  const effectiveConsumptionBps =
    audioPcmBytes !== null && integrity?.pcmMs
      ? Math.round((audioPcmBytes * 1000) / integrity.pcmMs)
      : null;

  if (expectedPacketCount === null || expectedPacketCount <= 0) {
    errors.push("transport summary missing expected_packet_count");
  }
  if (audioSent === null) {
    errors.push("transport summary missing audio_sent");
  } else if (expectedPacketCount !== null && audioSent !== expectedPacketCount) {
    errors.push(`audio_sent=${audioSent} does not equal expected_packet_count=${expectedPacketCount}`);
  }
  if (durationMs === null) {
    errors.push("transport duration is missing");
  } else {
    if (durationMs < args.minDurationMs) {
      errors.push(`recording duration ${durationMs} ms is below ${args.minDurationMs} ms`);
    }
    if (durationMs > args.maxDurationMs) {
      errors.push(`recording duration ${durationMs} ms exceeds ${args.maxDurationMs} ms`);
    }
  }
  if (!integrity) {
    errors.push("missing record session capture integrity line for steady consumption evidence");
  } else {
    if (integrity.captureBackpressureGapMs !== 0) {
      errors.push(`capture_backpressure_gap_ms=${integrity.captureBackpressureGapMs} must be 0`);
    }
    if (integrity.captureBackpressurePauseFrames !== 0) {
      errors.push(
        `capture_backpressure_pause_frames=${integrity.captureBackpressurePauseFrames} must be 0`,
      );
    }
    if (effectiveConsumptionBps !== null && effectiveConsumptionBps < 32000) {
      errors.push(`effective steady consumption ${effectiveConsumptionBps} B/s is below 32000 B/s`);
    }
  }
  if (poolHighWaterPct === null) {
    errors.push("transport summary missing pool_high_water_pct");
  } else if (poolHighWaterPct > args.maxPoolHighWaterPct) {
    errors.push(
      `pool_high_water_pct=${poolHighWaterPct} exceeds ${args.maxPoolHighWaterPct}`,
    );
  }
  if (audioFailed !== 0 || queueFull !== 0 || poolAllocFailed !== 0 || notifyFailed !== 0) {
    errors.push(
      `transport failure counters must be zero: audio_failed=${audioFailed} queue_full=${queueFull} pool_alloc_failed=${poolAllocFailed} notify_failed=${notifyFailed}`,
    );
  }
  if (audioSent !== null && audioSent > 0) {
    if (retryMbuf / audioSent > args.maxRetryRatio) {
      errors.push(`retry_mbuf=${retryMbuf} exceeds ${args.maxRetryRatio * 100}% of audio_sent=${audioSent}`);
    }
    if (retryEnomem / audioSent > args.maxRetryRatio) {
      errors.push(
        `retry_enomem=${retryEnomem} exceeds ${args.maxRetryRatio * 100}% of audio_sent=${audioSent}`,
      );
    }
  }
  if (lastDropReason !== null && lastDropReason !== "none") {
    errors.push(`last_drop_reason=${lastDropReason}`);
  }

  return {
    session: summary.session,
    line: summary.line,
    expectedPacketCount,
    audioSent,
    audioPcmBytes,
    elapsedMs,
    durationMs,
    retryMbuf,
    retryEnomem,
    poolHighWaterPct,
    audioFailed,
    queueFull,
    poolAllocFailed,
    notifyFailed,
    lastDropReason,
    effectiveConsumptionBps,
    integrity,
  };
}

const typeFacts = args.typeLog ? parseTypeLog(readText(args.typeLog), transport?.session, captureWindow) : null;
const capsuleFacts = args.capsuleLog ? parseCapsuleLog(readText(args.capsuleLog), captureWindow) : null;

if (!args.typeLog) {
  errors.push("missing --type-log evidence for Type packets and missing_packets");
} else if (!typeFacts?.hasCompleteSession) {
  errors.push("Type log has no complete session received evidence");
} else {
  if (typeFacts.missingPackets !== 0) {
    errors.push(`Type missing_packets=${typeFacts.missingPackets} must be 0`);
  }
  if (!typeFacts.sawFinal) {
    errors.push("Type log has no embedded_audio_final event in the capture window");
  }
  if (
    transport?.audioSent !== undefined &&
    transport.audioSent !== null &&
    typeFacts.packets !== null &&
    typeFacts.packets !== transport.audioSent
  ) {
    errors.push(`Type packets=${typeFacts.packets} does not equal firmware audio_sent=${transport.audioSent}`);
  }
}

if (!args.capsuleLog) {
  errors.push("missing --capsule-log evidence for non-empty final text");
} else if (!capsuleFacts?.hasDoneWithInsertedChars) {
  errors.push("capsule log has no done event with insertedChars > 0");
}

function inferCaptureWindow(serialPath, text) {
  const explicitStart = args.captureStartIso ? Date.parse(args.captureStartIso) : null;
  const explicitEnd = args.captureEndIso ? Date.parse(args.captureEndIso) : null;
  if (explicitStart !== null || explicitEnd !== null) {
    if (!Number.isFinite(explicitStart) || !Number.isFinite(explicitEnd)) {
      errors.push("capture-start-iso and capture-end-iso must both be valid ISO timestamps");
      return null;
    }
    return {
      source: "explicit",
      startUnixMs: explicitStart - args.timeSlackMs,
      endUnixMs: explicitEnd + args.timeSlackMs,
      nominalStartUnixMs: explicitStart,
      nominalEndUnixMs: explicitEnd,
      slackMs: args.timeSlackMs,
    };
  }
  const captureOnly = /capture_only_ms=(\d+)/.exec(text);
  if (!captureOnly) {
    errors.push("serial log has no capture_only_ms marker, so Type/capsule evidence cannot be time-correlated");
    return null;
  }
  const captureMs = Number.parseInt(captureOnly[1], 10);
  const endUnixMs = statSync(serialPath).mtimeMs;
  return {
    source: "serial_mtime_minus_capture_only_ms",
    captureOnlyMs: captureMs,
    startUnixMs: endUnixMs - captureMs - args.timeSlackMs,
    endUnixMs: endUnixMs + args.timeSlackMs,
    nominalStartUnixMs: endUnixMs - captureMs,
    nominalEndUnixMs: endUnixMs,
    slackMs: args.timeSlackMs,
  };
}

function parseLineTimestamp(line) {
  const match = /^(\d{4}-\d{2}-\d{2}T[^\s]+)/.exec(line);
  if (!match) {
    return null;
  }
  const unixMs = Date.parse(match[1]);
  return Number.isFinite(unixMs) ? unixMs : null;
}

function inCaptureWindow(unixMs, window) {
  if (!window) {
    return false;
  }
  return unixMs !== null && unixMs >= window.startUnixMs && unixMs <= window.endUnixMs;
}

function parseTypeLog(text, preferredSession, window) {
  const completeSessions = [];
  let missingPackets = null;
  let sawFinal = false;
  let linesInWindow = 0;
  for (const line of text.split(/\r?\n/)) {
    const unixMs = parseLineTimestamp(line);
    if (!inCaptureWindow(unixMs, window)) {
      continue;
    }
    linesInWindow += 1;
    const complete = /complete session received;.*session_id=Some\((\d+)\), pcm_bytes=(\d+), packets=(\d+)/.exec(
      line,
    );
    if (complete) {
      completeSessions.push({
        session: Number.parseInt(complete[1], 10),
        pcmBytes: Number.parseInt(complete[2], 10),
        packets: Number.parseInt(complete[3], 10),
      });
    }
    const missing = /background session completed .*missing_packets=(\d+)/.exec(line);
    if (missing) {
      missingPackets = Number.parseInt(missing[1], 10);
    }
    if (/embedded_audio_final/.test(line)) {
      sawFinal = true;
    }
  }
  const selected =
    completeSessions.find((session) => preferredSession !== null && session.session === preferredSession) ??
    completeSessions.at(-1) ??
    null;
  return {
    hasCompleteSession: selected !== null,
    session: selected?.session ?? null,
    pcmBytes: selected?.pcmBytes ?? null,
    packets: selected?.packets ?? null,
    missingPackets,
    sawFinal,
    linesInWindow,
    windowApplied: window !== null,
  };
}

function parseCapsuleLog(text, window) {
  let doneInsertedChars = 0;
  let doneEvents = 0;
  let eventsInWindow = 0;
  for (const line of text.split(/\r?\n/)) {
    const trimmed = line.trim();
    if (!trimmed.startsWith("{")) {
      continue;
    }
    try {
      const event = JSON.parse(trimmed);
      const unixMs = Date.parse(event.ts);
      if (!inCaptureWindow(Number.isFinite(unixMs) ? unixMs : null, window)) {
        continue;
      }
      eventsInWindow += 1;
      if (String(event.state).toLowerCase() === "done") {
        doneEvents += 1;
        const insertedChars = toInt(event.insertedChars);
        if (insertedChars !== null && insertedChars > doneInsertedChars) {
          doneInsertedChars = insertedChars;
        }
      }
    } catch {
      warnings.push("capsule log contains a non-JSON line that was ignored");
    }
  }
  return {
    doneEvents,
    doneInsertedChars,
    hasDoneWithInsertedChars: doneInsertedChars > 0,
    eventsInWindow,
    windowApplied: window !== null,
  };
}

const report = {
  status: errors.length === 0 ? "PASS" : "NO_GO",
  serial_log: args.serialLog,
  prompt_json: args.promptJson,
  type_log: args.typeLog ?? null,
  capsule_log: args.capsuleLog ?? null,
  thresholds: {
    min_duration_ms: args.minDurationMs,
    max_duration_ms: args.maxDurationMs,
    max_press_to_control_ms: args.maxPressToControlMs,
    max_pool_high_water_pct: args.maxPoolHighWaterPct,
    max_retry_ratio: args.maxRetryRatio,
    time_slack_ms: args.timeSlackMs,
  },
  capture_window: captureWindow
    ? {
        source: captureWindow.source,
        capture_only_ms: captureWindow.captureOnlyMs ?? null,
        nominal_start_iso: new Date(captureWindow.nominalStartUnixMs).toISOString(),
        nominal_end_iso: new Date(captureWindow.nominalEndUnixMs).toISOString(),
        start_iso: new Date(captureWindow.startUnixMs).toISOString(),
        end_iso: new Date(captureWindow.endUnixMs).toISOString(),
      }
    : null,
  prompt: prompt
    ? {
        selected: prompt.selected ?? null,
        has_spoken_text: typeof prompt.spoken_text === "string" && prompt.spoken_text.trim().length > 0,
        note_chars: typeof prompt.text === "string" ? prompt.text.length : null,
      }
    : null,
  dispatches,
  active_stop_events: activeStops,
  transport,
  type: typeFacts,
  capsule: capsuleFacts,
  warnings,
  errors,
};

const rendered = `${JSON.stringify(report, null, 2)}\n`;
if (args.outputJson) {
  mkdirSync(dirname(args.outputJson), { recursive: true });
  writeFileSync(args.outputJson, rendered, "utf8");
}
process.stdout.write(rendered);
process.exit(report.status === "PASS" ? 0 : 1);
