#!/usr/bin/env node
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const DEFAULT_THRESHOLDS = Object.freeze({
  requiredSamples: 20,
  wakeP95Ms: 1_200,
  wakeMaxMs: 1_500,
  phraseTailP95Ms: 350,
  phraseTailMaxMs: 500,
  previewP95Ms: 1_500,
  previewMaxMs: 1_800,
  endpointMinMs: 900,
  endpointMaxMs: 1_100,
  stopToDoneP95Ms: 500,
});

function parseArgs(argv) {
  const parsed = { ...DEFAULT_THRESHOLDS };
  const numeric = new Set([
    "requiredSamples",
    "wakeP95Ms",
    "wakeMaxMs",
    "phraseTailP95Ms",
    "phraseTailMaxMs",
    "previewP95Ms",
    "previewMaxMs",
    "endpointMinMs",
    "endpointMaxMs",
    "stopToDoneP95Ms",
  ]);
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (!arg.startsWith("--")) failUsage(`Unexpected argument: ${arg}`);
    const key = arg.slice(2).replace(/-([a-z])/g, (_, ch) => ch.toUpperCase());
    const value = argv[index + 1];
    if (value === undefined || value.startsWith("--")) failUsage(`Missing value for ${arg}`);
    index += 1;
    parsed[key] = numeric.has(key) ? Number.parseInt(value, 10) : value;
  }
  if (!parsed.log) failUsage("Missing --log");
  for (const key of numeric) {
    if (!Number.isFinite(parsed[key]) || parsed[key] < 0) failUsage(`Invalid --${key}`);
  }
  if (parsed.after && !Number.isFinite(Date.parse(parsed.after))) failUsage("Invalid --after ISO timestamp");
  return parsed;
}

function failUsage(message) {
  console.error(
    `${message}\n\nUsage: node scripts/check-live-wake-endpoint.mjs --log <listener-type.log> ` +
      "[--after <ISO>] [--required-samples 20] [--output-json <report.json>]",
  );
  process.exit(2);
}

function parseTimestamp(line) {
  const match = /^(\d{4}-\d{2}-\d{2}T\S+?Z)\s/.exec(line);
  if (!match) return null;
  const value = Date.parse(match[1]);
  return Number.isFinite(value) ? value : null;
}

function integerField(line, name) {
  const match = new RegExp(`(?:^|\\s)${name}=(\\d+)`).exec(line);
  return match ? Number.parseInt(match[1], 10) : null;
}

function stringField(line, name) {
  const match = new RegExp(`(?:^|\\s)${name}=([^\\s]+)`).exec(line);
  return match ? match[1] : null;
}

function jsonIntegerField(line, name) {
  const match = new RegExp(`"${name}":(\\d+)`).exec(line);
  return match ? Number.parseInt(match[1], 10) : null;
}

function percentile(values, fraction) {
  if (values.length === 0) return null;
  const sorted = [...values].sort((left, right) => left - right);
  return sorted[Math.max(0, Math.ceil(sorted.length * fraction) - 1)];
}

function linkCoordinatorToEmbedded(line, coordinatorToEmbedded) {
  const match = /session_id=Some\(([0-9a-f-]{36})\).*embedded_session_id=(\d+)/.exec(line);
  if (!match) return;
  coordinatorToEmbedded.set(match[1], Number.parseInt(match[2], 10));
}

export function analyzeLiveWakeLog(text, options = {}) {
  const thresholds = Object.fromEntries(
    Object.keys(DEFAULT_THRESHOLDS).map(key => [key, options[key] ?? DEFAULT_THRESHOLDS[key]]),
  );
  const afterMs = options.after ? Date.parse(options.after) : Number.NEGATIVE_INFINITY;
  const samples = new Map();
  const coordinatorToEmbedded = new Map();
  const endpoints = new Map();
  const settledTargets = new Map();
  const previewLatencies = new Map();
  const previewGrowth = new Map();
  const firmwareStopOrigins = new Map();
  const finalIntegrity = new Map();
  const completions = new Map();
  const doneSessions = new Set();
  const transportCompletions = new Map();
  const transportFaults = [];
  let pendingOwnerMode = null;
  let pendingTransportEmbeddedId = null;
  let activeCoordinatorSessionId = null;
  let pendingOwnerSafeRestore = null;

  for (const [lineIndex, line] of text.split(/\r?\n/).entries()) {
    const timestampMs = parseTimestamp(line);
    if (timestampMs !== null && timestampMs < afterMs) continue;
    linkCoordinatorToEmbedded(line, coordinatorToEmbedded);

    if (line.includes("owner not enrolled") || line.includes("owner template inactive")) {
      pendingOwnerMode = { enrolled: false, timestampMs };
    } else if (line.includes("compared enrolled_templates=")) {
      pendingOwnerMode = { enrolled: true, timestampMs };
    }

    if (line.includes("live automatic session activated and released")) {
      const embeddedId = integerField(line, "embedded_session_id");
      if (embeddedId !== null) {
        const ownerModeFresh = pendingOwnerMode
          && timestampMs !== null
          && pendingOwnerMode.timestampMs !== null
          && timestampMs - pendingOwnerMode.timestampMs >= 0
          && timestampMs - pendingOwnerMode.timestampMs <= 3_000;
        samples.set(embeddedId, {
          embeddedSessionId: embeddedId,
          coordinatorSessionId: null,
          timestamp: timestampMs === null ? null : new Date(timestampMs).toISOString(),
          wakeToCapsuleMs: integerField(line, "wake_to_capsule_request_ms"),
          phraseTailToCapsuleMs: integerField(line, "phrase_tail_to_capsule_ms"),
          ownerEnrolled: ownerModeFresh ? pendingOwnerMode.enrolled : null,
          endpointTimeoutMs: null,
          endpointReason: null,
          firmwareStopOrigin: null,
          bodyStarted: null,
          sentencePause: null,
          semanticContinuation: null,
          settledTargetToEndpointMs: null,
          previewLatencyMs: null,
          previewInflationBursts: 0,
          providerFinalChars: null,
          resultFinalChars: null,
          ownerSafeRestore: null,
          stopToDoneMs: null,
          insertionStatus: null,
          done: false,
          pcmBytes: null,
          missingPackets: null,
          faults: [],
          errors: [],
          line: lineIndex + 1,
        });
      }
    }

    if (line.includes("stop_to_transcribing_ms=")) {
      const sessionId = stringField(line, "session_id");
      if (sessionId) {
        endpoints.set(sessionId, {
          reason: stringField(line, "reason"),
          timeoutMs: integerField(line, "timeout_ms"),
          bodyStarted: stringField(line, "body_started"),
          sentencePause: stringField(line, "sentence_pause"),
          semanticContinuation: stringField(line, "semantic_continuation"),
          timestampMs,
          settledAtMs: settledTargets.get(sessionId) ?? null,
        });
      }
    }

    const activeSession = /session_id=Some\(([0-9a-f-]{36})\).*\bble_start\b/.exec(line);
    if (activeSession) activeCoordinatorSessionId = activeSession[1];

    const partial = /event=asr_partial\b.*\bchars=(\d+)/.exec(line);
    if (partial && activeCoordinatorSessionId) {
      const growth = previewGrowth.get(activeCoordinatorSessionId) ?? {
        lastPreviewChars: null,
        lastProviderChars: null,
        pendingPreviewChars: null,
        consecutiveInflation: 0,
        maxConsecutiveInflation: 0,
      };
      growth.pendingPreviewChars = Number.parseInt(partial[1], 10);
      previewGrowth.set(activeCoordinatorSessionId, growth);
    }

    if (line.includes('"has_final_frame":false') && activeCoordinatorSessionId) {
      const providerChars = jsonIntegerField(line, "provider_result_chars");
      const growth = previewGrowth.get(activeCoordinatorSessionId);
      if (growth?.pendingPreviewChars !== null && providerChars !== null) {
        if (growth.lastPreviewChars !== null && growth.lastProviderChars !== null) {
          const previewDelta = growth.pendingPreviewChars - growth.lastPreviewChars;
          const providerDelta = providerChars - growth.lastProviderChars;
          const inflated = previewDelta >= 10 && previewDelta > Math.max(8, providerDelta + 8);
          growth.consecutiveInflation = inflated ? growth.consecutiveInflation + 1 : 0;
          growth.maxConsecutiveInflation = Math.max(
            growth.maxConsecutiveInflation,
            growth.consecutiveInflation,
          );
        }
        growth.lastPreviewChars = growth.pendingPreviewChars;
        growth.lastProviderChars = providerChars;
        growth.pendingPreviewChars = null;
      }
    }

    if (line.includes("[asr] target-speaker state") && activeCoordinatorSessionId) {
      const pending = stringField(line, "pending_provisional");
      const targetAdvanced = stringField(line, "target_advanced");
      if (pending === "true") {
        settledTargets.delete(activeCoordinatorSessionId);
      } else if (pending === "false" && targetAdvanced === "true" && timestampMs !== null) {
        settledTargets.set(activeCoordinatorSessionId, timestampMs);
      }
    }

    if (line.includes('"event":"embedded_audio_preview_first_provider_stream"')) {
      const latencyMs = jsonIntegerField(line, "timing_value_ms");
      if (activeCoordinatorSessionId && latencyMs !== null) {
        previewLatencies.set(activeCoordinatorSessionId, latencyMs);
      }
    }

    if (line.includes("protocol final restores owner-safe provider text after diarization regression")) {
      pendingOwnerSafeRestore = {
        targetChars: integerField(line, "target_chars"),
        providerChars: integerField(line, "provider_chars"),
      };
    }

    if (line.includes('"has_final_frame":true')) {
      const providerFinalChars = jsonIntegerField(line, "provider_result_chars");
      const resultFinalChars = jsonIntegerField(line, "result_chars");
      if (activeCoordinatorSessionId) {
        finalIntegrity.set(activeCoordinatorSessionId, {
          providerFinalChars,
          resultFinalChars,
          ownerSafeRestore: pendingOwnerSafeRestore,
        });
      }
      pendingOwnerSafeRestore = null;
    }

    if (line.includes("event=stop") && line.includes("embedded_session_id=")) {
      const embeddedId = integerField(line, "embedded_session_id");
      const origin = stringField(line, "origin");
      if (embeddedId !== null && origin) {
        firmwareStopOrigins.set(embeddedId, {
          origin,
          timestampMs,
          coordinatorSessionId: activeCoordinatorSessionId,
          settledAtMs: activeCoordinatorSessionId
            ? settledTargets.get(activeCoordinatorSessionId) ?? null
            : null,
        });
      }
    }

    if (line.includes("[coord] stop_to_done_ms=")) {
      const sessionId = stringField(line, "session_id");
      if (sessionId) {
        completions.set(sessionId, {
          stopToDoneMs: integerField(line, "stop_to_done_ms"),
          insertionStatus: stringField(line, "insertion_status"),
        });
      }
    }

    if (line.includes("state=Done")) {
      const sessionId = stringField(line, "session_id");
      if (sessionId) doneSessions.add(sessionId);
    }

    const transport = /capture #\d+: complete session received;.*session_id=Some\((\d+)\), pcm_bytes=(\d+)/.exec(line);
    if (transport) {
      pendingTransportEmbeddedId = Number.parseInt(transport[1], 10);
      transportCompletions.set(pendingTransportEmbeddedId, {
        pcmBytes: Number.parseInt(transport[2], 10),
        missingPackets: null,
      });
    }
    const background = /background session completed while keeping notify open pcm_bytes=(\d+) missing_packets=(\d+)/.exec(line);
    if (background && pendingTransportEmbeddedId !== null) {
      transportCompletions.set(pendingTransportEmbeddedId, {
        pcmBytes: Number.parseInt(background[1], 10),
        missingPackets: Number.parseInt(background[2], 10),
      });
      pendingTransportEmbeddedId = null;
    }

    if (/QueueFull|queue_full=[1-9]\d*|notify_failure=[1-9]\d*|notify_failed=[1-9]\d*|pool_exhaustion=[1-9]\d*|pool_alloc_failed=[1-9]\d*/i.test(line)) {
      transportFaults.push({ timestampMs, line: lineIndex + 1, category: "transport_counter_nonzero" });
    }
  }

  for (const [coordinatorId, embeddedId] of coordinatorToEmbedded) {
    const sample = samples.get(embeddedId);
    if (!sample) continue;
    sample.coordinatorSessionId = coordinatorId;
    const endpoint = endpoints.get(coordinatorId);
    if (endpoint) {
      sample.endpointReason = endpoint.reason;
      sample.endpointTimeoutMs = endpoint.timeoutMs;
      sample.bodyStarted = endpoint.bodyStarted === "true";
      sample.sentencePause = endpoint.sentencePause === "true";
      sample.semanticContinuation = endpoint.semanticContinuation === "true";
      if (endpoint.settledAtMs !== null && endpoint.timestampMs !== null) {
        sample.settledTargetToEndpointMs = Math.max(0, endpoint.timestampMs - endpoint.settledAtMs);
      }
    }
    sample.previewLatencyMs = previewLatencies.get(coordinatorId) ?? null;
    sample.previewInflationBursts =
      previewGrowth.get(coordinatorId)?.maxConsecutiveInflation ?? 0;
    const integrity = finalIntegrity.get(coordinatorId);
    if (integrity) {
      sample.providerFinalChars = integrity.providerFinalChars;
      sample.resultFinalChars = integrity.resultFinalChars;
      sample.ownerSafeRestore = integrity.ownerSafeRestore;
    }
    const completion = completions.get(coordinatorId);
    if (completion) {
      sample.stopToDoneMs = completion.stopToDoneMs;
      sample.insertionStatus = completion.insertionStatus;
    }
    sample.done = doneSessions.has(coordinatorId);
  }

  for (const [embeddedId, firmwareStop] of firmwareStopOrigins) {
    const sample = samples.get(embeddedId);
    if (!sample) continue;
    sample.firmwareStopOrigin = firmwareStop.origin;
    if (firmwareStop.settledAtMs !== null && firmwareStop.timestampMs !== null) {
      const firmwareLatencyMs = Math.max(0, firmwareStop.timestampMs - firmwareStop.settledAtMs);
      sample.settledTargetToEndpointMs = sample.settledTargetToEndpointMs === null
        ? firmwareLatencyMs
        : Math.min(sample.settledTargetToEndpointMs, firmwareLatencyMs);
    }
  }

  for (const [embeddedId, transport] of transportCompletions) {
    const sample = samples.get(embeddedId);
    if (!sample) continue;
    sample.pcmBytes = transport.pcmBytes;
    sample.missingPackets = transport.missingPackets;
  }

  const ordered = [...samples.values()].sort((left, right) => (left.timestamp ?? "").localeCompare(right.timestamp ?? ""));
  for (const sample of ordered) {
    const startedMs = sample.timestamp ? Date.parse(sample.timestamp) : null;
    const next = ordered.find(candidate => candidate.timestamp && sample.timestamp && candidate.timestamp > sample.timestamp);
    const endedMs = next?.timestamp ? Date.parse(next.timestamp) : Number.POSITIVE_INFINITY;
    sample.faults = transportFaults.filter(fault => {
      if (fault.timestampMs === null || startedMs === null) return false;
      return fault.timestampMs >= startedMs && fault.timestampMs < endedMs;
    });
    const required = [
      [sample.coordinatorSessionId, "missing coordinator session link"],
      [sample.wakeToCapsuleMs, "missing wake_to_capsule_request_ms"],
      [sample.phraseTailToCapsuleMs, "missing phrase_tail_to_capsule_ms"],
      [sample.previewLatencyMs, "missing first provider preview latency"],
      [sample.stopToDoneMs, "missing stop_to_done_ms"],
      [sample.insertionStatus, "missing insertion status"],
      [sample.pcmBytes, "missing BLE completion"],
      [sample.missingPackets, "missing BLE packet result"],
      [sample.providerFinalChars, "missing provider final integrity"],
      [sample.resultFinalChars, "missing retained final integrity"],
      [sample.settledTargetToEndpointMs, "missing settled-target endpoint latency"],
    ];
    for (const [value, error] of required) if (value === null) sample.errors.push(error);
    if (sample.wakeToCapsuleMs > thresholds.wakeMaxMs) sample.errors.push(`wake latency exceeds ${thresholds.wakeMaxMs} ms`);
    if (sample.phraseTailToCapsuleMs > thresholds.phraseTailMaxMs) sample.errors.push(`phrase-tail latency exceeds ${thresholds.phraseTailMaxMs} ms`);
    if (sample.previewLatencyMs > thresholds.previewMaxMs) sample.errors.push(`first preview latency exceeds ${thresholds.previewMaxMs} ms`);
    if (sample.previewInflationBursts >= 2) {
      sample.errors.push("streaming preview repeatedly outgrew the provider revision window");
    }
    const hostAutomaticEndpoint = sample.endpointReason === "target_speaker_inactive_1000ms";
    const firmwareAutomaticEndpoint = sample.firmwareStopOrigin === "VoiceActivation";
    if (!hostAutomaticEndpoint && !firmwareAutomaticEndpoint) sample.errors.push("missing automatic endpoint evidence");
    if (sample.settledTargetToEndpointMs > thresholds.endpointMaxMs) {
      sample.errors.push(`settled target endpoint exceeds ${thresholds.endpointMaxMs} ms`);
    }
    if (hostAutomaticEndpoint) {
      if (sample.endpointTimeoutMs < thresholds.endpointMinMs || sample.endpointTimeoutMs > thresholds.endpointMaxMs) sample.errors.push("automatic endpoint timeout outside tolerance");
      if (sample.bodyStarted !== true) sample.errors.push("automatic endpoint fired before dictated body started");
      if (sample.semanticContinuation === true) sample.errors.push("automatic endpoint cut a semantic continuation");
    }
    if (sample.resultFinalChars !== null && sample.resultFinalChars <= 0) sample.errors.push("retained provider final is empty");
    if (sample.ownerSafeRestore) {
      if (sample.ownerSafeRestore.providerChars !== sample.providerFinalChars
          || sample.providerFinalChars !== sample.resultFinalChars) {
        sample.errors.push("owner-safe provider tail was not preserved into final result");
      }
    }
    if (sample.insertionStatus !== "Inserted") sample.errors.push("final text was not inserted");
    if (!sample.done) sample.errors.push("capsule did not reach Done");
    if (sample.missingPackets !== 0) sample.errors.push("BLE missing_packets is nonzero");
    if (sample.faults.length > 0) sample.errors.push("transport failure counter became nonzero");
  }

  const wakeValues = ordered.map(sample => sample.wakeToCapsuleMs).filter(Number.isFinite);
  const phraseTailValues = ordered.map(sample => sample.phraseTailToCapsuleMs).filter(Number.isFinite);
  const previewValues = ordered.map(sample => sample.previewLatencyMs).filter(Number.isFinite);
  const doneValues = ordered.map(sample => sample.stopToDoneMs).filter(Number.isFinite);
  const aggregate = {
    wakeToCapsuleP95Ms: percentile(wakeValues, 0.95),
    wakeToCapsuleMaxMs: wakeValues.length ? Math.max(...wakeValues) : null,
    phraseTailToCapsuleP95Ms: percentile(phraseTailValues, 0.95),
    phraseTailToCapsuleMaxMs: phraseTailValues.length ? Math.max(...phraseTailValues) : null,
    previewP95Ms: percentile(previewValues, 0.95),
    previewMaxMs: previewValues.length ? Math.max(...previewValues) : null,
    stopToDoneP95Ms: percentile(doneValues, 0.95),
  };
  const failures = [];
  for (const sample of ordered) {
    for (const error of sample.errors) failures.push(`embedded_session_id=${sample.embeddedSessionId}: ${error}`);
  }
  const hasRequiredPopulation = ordered.length >= thresholds.requiredSamples;
  // Percentiles are population gates. A short operator spot-check may be
  // within the hard per-session ceiling without representing p95; keep it
  // INCOMPLETE instead of falsely turning one accepted outlier into NO_GO.
  if (hasRequiredPopulation && aggregate.wakeToCapsuleP95Ms > thresholds.wakeP95Ms) failures.push(`wake p95 exceeds ${thresholds.wakeP95Ms} ms`);
  if (aggregate.wakeToCapsuleMaxMs > thresholds.wakeMaxMs) failures.push(`wake max exceeds ${thresholds.wakeMaxMs} ms`);
  if (hasRequiredPopulation && aggregate.phraseTailToCapsuleP95Ms > thresholds.phraseTailP95Ms) failures.push(`phrase-tail p95 exceeds ${thresholds.phraseTailP95Ms} ms`);
  if (aggregate.phraseTailToCapsuleMaxMs > thresholds.phraseTailMaxMs) failures.push(`phrase-tail max exceeds ${thresholds.phraseTailMaxMs} ms`);
  if (hasRequiredPopulation && aggregate.previewP95Ms > thresholds.previewP95Ms) failures.push(`preview p95 exceeds ${thresholds.previewP95Ms} ms`);
  if (aggregate.previewMaxMs > thresholds.previewMaxMs) failures.push(`preview max exceeds ${thresholds.previewMaxMs} ms`);
  if (hasRequiredPopulation && aggregate.stopToDoneP95Ms > thresholds.stopToDoneP95Ms) failures.push(`stop-to-done p95 exceeds ${thresholds.stopToDoneP95Ms} ms`);

  const status = failures.length > 0
    ? "NO_GO"
    : ordered.length < thresholds.requiredSamples
      ? "INCOMPLETE"
      : "PASS";
  return {
    schemaVersion: 1,
    status,
    after: options.after ?? null,
    thresholds,
    sampleCount: ordered.length,
    requiredSamples: thresholds.requiredSamples,
    enrolledOwnerSamples: ordered.filter(sample => sample.ownerEnrolled === true).length,
    openGateSamples: ordered.filter(sample => sample.ownerEnrolled === false).length,
    aggregate,
    failures,
    samples: ordered,
  };
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  const report = analyzeLiveWakeLog(readFileSync(args.log, "utf8"), args);
  const output = `${JSON.stringify(report, null, 2)}\n`;
  if (args.outputJson) {
    mkdirSync(dirname(resolve(args.outputJson)), { recursive: true });
    writeFileSync(args.outputJson, output, "utf8");
  }
  process.stdout.write(output);
  process.exitCode = report.status === "PASS" ? 0 : report.status === "INCOMPLETE" ? 2 : 1;
}

if (resolve(process.argv[1] ?? "") === fileURLToPath(import.meta.url)) main();
