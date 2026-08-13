#!/usr/bin/env node
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const DEFAULT_THRESHOLDS = Object.freeze({
  requiredAttempts: 20,
  requiredSuccesses: 19,
  wakeP95Ms: 1_000,
  wakeMaxMs: 1_200,
  phraseTailP95Ms: 350,
  phraseTailMaxMs: 500,
  bodyWaitMinMs: 3_000,
});

function failUsage(message) {
  console.error(
    `${message}\n\nUsage: node scripts/check-live-wake-only.mjs --log <listener-type.log> ` +
      "[--log <rotated.log>] [--after <ISO>] [--before <ISO>] " +
      "(--attempt-ids <id,id,...> | --attempt-markers <attempt-markers.json>) " +
      "[--required-attempts 20] [--required-successes 19] [--output-json <report.json>]",
  );
  process.exit(2);
}

function parseArgs(argv) {
  const parsed = { ...DEFAULT_THRESHOLDS, logs: [] };
  const numeric = new Set([
    "requiredAttempts",
    "requiredSuccesses",
    "wakeP95Ms",
    "wakeMaxMs",
    "phraseTailP95Ms",
    "phraseTailMaxMs",
    "bodyWaitMinMs",
  ]);
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (!arg.startsWith("--")) failUsage(`Unexpected argument: ${arg}`);
    const value = argv[index + 1];
    if (value === undefined || value.startsWith("--")) failUsage(`Missing value for ${arg}`);
    index += 1;
    const key = arg.slice(2).replace(/-([a-z])/g, (_, ch) => ch.toUpperCase());
    if (key === "log") parsed.logs.push(value);
    else parsed[key] = numeric.has(key) ? Number.parseInt(value, 10) : value;
  }
  if (parsed.logs.length === 0) failUsage("Missing --log");
  for (const key of numeric) {
    if (!Number.isFinite(parsed[key]) || parsed[key] < 0) failUsage(`Invalid --${key}`);
  }
  if (parsed.requiredSuccesses > parsed.requiredAttempts) {
    failUsage("--required-successes cannot exceed --required-attempts");
  }
  if (parsed.attemptIds !== undefined && !/^\d+(?:,\d+)*$/.test(parsed.attemptIds)) {
    failUsage("Invalid --attempt-ids");
  }
  if (parsed.attemptIds === undefined && typeof parsed.attemptMarkers !== "string") {
    failUsage("Real attempts must be explicitly labeled with --attempt-ids or --attempt-markers");
  }
  parsed.attemptIds = parsed.attemptIds === undefined
    ? []
    : parsed.attemptIds.split(",").map(value => Number.parseInt(value, 10));
  if (new Set(parsed.attemptIds).size !== parsed.attemptIds.length) {
    failUsage("--attempt-ids must not contain duplicates");
  }
  for (const key of ["after", "before"]) {
    if (parsed[key] && !Number.isFinite(Date.parse(parsed[key]))) failUsage(`Invalid --${key} ISO timestamp`);
  }
  if (parsed.after && parsed.before && Date.parse(parsed.after) >= Date.parse(parsed.before)) {
    failUsage("--after must be earlier than --before");
  }
  return parsed;
}

function timestampMs(line) {
  const match = /^(\d{4}-\d{2}-\d{2}T\S+?Z)\s/.exec(line);
  if (!match) return null;
  const parsed = Date.parse(match[1]);
  return Number.isFinite(parsed) ? parsed : null;
}

function integerField(line, name) {
  const match = new RegExp(`(?:^|\\s)${name}=(\\d+)`).exec(line);
  return match ? Number.parseInt(match[1], 10) : null;
}

function numberField(line, name) {
  const match = new RegExp(`(?:^|\\s)${name}=([0-9]+(?:\\.[0-9]+)?)`).exec(line);
  return match ? Number.parseFloat(match[1]) : null;
}

function percentile(values, fraction) {
  if (values.length === 0) return null;
  const ordered = [...values].sort((left, right) => left - right);
  return ordered[Math.max(0, Math.ceil(ordered.length * fraction) - 1)];
}

function sessionIdFromLine(line) {
  const match = /session_id=(?:Some\()?([0-9a-f-]{36})\)?/i.exec(line);
  return match?.[1] ?? null;
}

function detailSessionId(line) {
  const match = /"sessionId":"([0-9a-f-]{36})"/i.exec(line);
  return match?.[1] ?? null;
}

function orderedLines(text) {
  return text
    .split(/\r?\n/)
    .map((line, sourceIndex) => ({ line, sourceIndex, timestampMs: timestampMs(line) }))
    .filter(entry => entry.line.length > 0)
    .sort((left, right) => {
      if (left.timestampMs === null && right.timestampMs === null) return left.sourceIndex - right.sourceIndex;
      if (left.timestampMs === null) return 1;
      if (right.timestampMs === null) return -1;
      return left.timestampMs - right.timestampMs || left.sourceIndex - right.sourceIndex;
    });
}

function newAttempt(embeddedSessionId, startedAtMs, line) {
  return {
    embeddedSessionId,
    coordinatorSessionId: null,
    startedAt: startedAtMs === null ? null : new Date(startedAtMs).toISOString(),
    startLine: line,
    duplicateStartCount: 0,
    decision: "pending",
    decisionReason: null,
    wakeEndMs: null,
    backendRecordingAtMs: null,
    frontendRecordingAtMs: null,
    frontendIdleAtMs: null,
    silentExpiryAtMs: null,
    wakeOnlyEventAtMs: null,
    pcmBytes: null,
    missingPackets: null,
    wakeToVisibleMs: null,
    phraseTailToVisibleMs: null,
    bodyWaitMs: null,
    classification: "pending",
    sideEffects: [],
    errors: [],
    success: false,
  };
}

function uniquePush(values, value) {
  if (!values.includes(value)) values.push(value);
}

function parseJsonFile(path) {
  return JSON.parse(readFileSync(path, "utf8").replace(/^\uFEFF/, ""));
}

function normalizedAttemptMarkers(value) {
  if (!Array.isArray(value)) return [];
  return value.map((item, index) => ({
    ordinal: Number.isInteger(item?.ordinal) ? item.ordinal : index + 1,
    markedAt: typeof item?.markedAt === "string" ? item.markedAt : null,
    associationEndedAt: typeof item?.associationEndedAt === "string" ? item.associationEndedAt : null,
    embeddedSessionId: Number.isInteger(item?.embeddedSessionId) ? item.embeddedSessionId : null,
  }));
}

function missingCandidateAttempt(marker) {
  const attempt = newAttempt(
    null,
    marker.markedAt === null ? null : Date.parse(marker.markedAt),
    null,
  );
  attempt.attemptOrdinal = marker.ordinal;
  attempt.markedAt = marker.markedAt;
  attempt.associationEndedAt = marker.associationEndedAt;
  attempt.decision = "missing";
  attempt.decisionReason = "no_voice_activation_candidate";
  attempt.classification = "missing_candidate";
  attempt.errors.push("no BLE voice-activation candidate observed after explicit operator marker");
  return attempt;
}

export function analyzeLiveWakeOnlyLog(text, options = {}) {
  const thresholds = Object.fromEntries(
    Object.keys(DEFAULT_THRESHOLDS).map(key => [key, options[key] ?? DEFAULT_THRESHOLDS[key]]),
  );
  const afterMs = options.after ? Date.parse(options.after) : Number.NEGATIVE_INFINITY;
  const beforeMs = options.before ? Date.parse(options.before) : Number.POSITIVE_INFINITY;
  const attempts = new Map();
  const coordinatorToEmbedded = new Map();
  const sessionEvidence = new Map();
  const pendingTransport = [];
  const transportFaults = [];
  const explicitAttemptIds = Array.isArray(options.attemptIds)
    ? options.attemptIds.filter(Number.isInteger)
    : typeof options.attemptIds === "string" && options.attemptIds.length > 0
      ? options.attemptIds.split(",").map(value => Number.parseInt(value, 10)).filter(Number.isInteger)
      : [];
  const labeledAttemptIds = new Set(explicitAttemptIds);
  const attemptMarkers = normalizedAttemptMarkers(options.attemptMarkers);
  const hasExplicitLabels = labeledAttemptIds.size > 0 || attemptMarkers.length > 0;

  function evidence(sessionId) {
    if (!sessionEvidence.has(sessionId)) {
      sessionEvidence.set(sessionId, {
        backendRecordingAtMs: null,
        frontendRecordingAtMs: null,
        frontendIdleAtMs: null,
        silentExpiryAtMs: null,
        wakeOnlyEventAtMs: null,
        sideEffects: [],
      });
    }
    return sessionEvidence.get(sessionId);
  }

  for (const entry of orderedLines(text)) {
    const { line, timestampMs: atMs, sourceIndex } = entry;
    if (atMs !== null && (atMs < afterMs || atMs >= beforeMs)) continue;

    const start = /event=start embedded_session_id=(\d+) origin=VoiceActivation/.exec(line);
    if (start) {
      const embeddedId = Number.parseInt(start[1], 10);
      if (attempts.has(embeddedId)) attempts.get(embeddedId).duplicateStartCount += 1;
      else attempts.set(embeddedId, newAttempt(embeddedId, atMs, sourceIndex + 1));
    }

    if (line.includes("[wake-phrase] live automatic session activated and released")) {
      const embeddedId = integerField(line, "embedded_session_id");
      if (embeddedId !== null) {
        const attempt = attempts.get(embeddedId) ?? newAttempt(embeddedId, atMs, sourceIndex + 1);
        attempts.set(embeddedId, attempt);
        attempt.decision = "accepted";
        attempt.decisionReason = "wake_phrase_match";
        const wakeEndSeconds = numberField(line, "wake_end_s");
        attempt.wakeEndMs = wakeEndSeconds === null ? null : Math.round(wakeEndSeconds * 1_000);
      }
    }

    if (line.includes("[wake-phrase] automatic candidate rejected embedded_session_id=")) {
      const embeddedId = integerField(line, "embedded_session_id");
      if (embeddedId !== null) {
        const attempt = attempts.get(embeddedId) ?? newAttempt(embeddedId, atMs, sourceIndex + 1);
        attempts.set(embeddedId, attempt);
        attempt.decision = "rejected";
        attempt.decisionReason = "wake_phrase_non_match";
      }
    }

    const link = /embedded_session_id=(\d+) coordinator_session_id=([0-9a-f-]{36})/i.exec(line);
    if (link) coordinatorToEmbedded.set(link[2], Number.parseInt(link[1], 10));

    let sessionId = sessionIdFromLine(line) ?? detailSessionId(line);
    if (sessionId) {
      const item = evidence(sessionId);
      if (/source=backend\.capsule event=emit_request .*state=Recording .*visible=true/.test(line)) {
        item.backendRecordingAtMs ??= atMs;
      }
      if (/source=frontend\.capsule event=event_received state=recording/.test(line)) {
        item.frontendRecordingAtMs ??= atMs;
      }
      if (/source=frontend\.capsule event=event_received state=idle/.test(line)) {
        item.frontendIdleAtMs ??= atMs;
      }
      if (line.includes("[coord] wake-only body window expired silently")) item.silentExpiryAtMs ??= atMs;
      if (line.includes("wake_only_expired transcript_empty=true")) item.wakeOnlyEventAtMs ??= atMs;
      if (/emptyTranscript|state=(?:Error|error)|event=error/i.test(line)) {
        uniquePush(item.sideEffects, "error_or_empty_transcript");
      }
      if (/\binsertion_status=(?:Inserted|ClipboardOnly|Failed)\b|\bclipboard=stored\b/.test(line)) {
        uniquePush(item.sideEffects, "text_or_clipboard_insertion");
      }
      if (/"insertedChars":(?!null)\d+/.test(line)) uniquePush(item.sideEffects, "frontend_inserted_chars");
    }

    const complete = /complete session received;.*session_id=Some\((\d+)\), pcm_bytes=(\d+)/.exec(line);
    if (complete) {
      const embeddedId = Number.parseInt(complete[1], 10);
      const attempt = attempts.get(embeddedId);
      if (attempt) attempt.pcmBytes = Number.parseInt(complete[2], 10);
      pendingTransport.push(embeddedId);
    }
    const background = /background session completed while keeping notify open pcm_bytes=(\d+) missing_packets=(\d+)/.exec(line);
    if (background && pendingTransport.length > 0) {
      const embeddedId = pendingTransport.shift();
      const attempt = attempts.get(embeddedId);
      if (attempt) {
        attempt.pcmBytes = Number.parseInt(background[1], 10);
        attempt.missingPackets = Number.parseInt(background[2], 10);
      }
    }

    if (/QueueFull|queue_full=[1-9]\d*|notify(?:_|\s)(?:failure|failed)(?:=[1-9]\d*)?|pool_(?:exhaustion|alloc_failed)=[1-9]\d*/i.test(line)) {
      transportFaults.push({
        timestamp: atMs === null ? null : new Date(atMs).toISOString(),
        line: sourceIndex + 1,
        category: /QueueFull/i.test(line) ? "queue_full" : "notify_or_transport_failure",
      });
    }
  }

  for (const [coordinatorId, embeddedId] of coordinatorToEmbedded) {
    const attempt = attempts.get(embeddedId);
    if (!attempt) continue;
    attempt.coordinatorSessionId = coordinatorId;
    const item = sessionEvidence.get(coordinatorId);
    if (!item) continue;
    Object.assign(attempt, item);
  }

  const observedCandidates = [...attempts.values()].sort((left, right) =>
    (left.startedAt ?? "").localeCompare(right.startedAt ?? "") || left.embeddedSessionId - right.embeddedSessionId,
  );
  const ordered = [];
  const selectedCandidateIds = new Set();
  for (const marker of attemptMarkers) {
    if (marker.embeddedSessionId === null || !attempts.has(marker.embeddedSessionId)) {
      ordered.push(missingCandidateAttempt(marker));
      continue;
    }
    const attempt = attempts.get(marker.embeddedSessionId);
    attempt.attemptOrdinal = marker.ordinal;
    attempt.markedAt = marker.markedAt;
    attempt.associationEndedAt = marker.associationEndedAt;
    ordered.push(attempt);
    selectedCandidateIds.add(marker.embeddedSessionId);
  }
  for (const attempt of observedCandidates) {
    if (!labeledAttemptIds.has(attempt.embeddedSessionId) || selectedCandidateIds.has(attempt.embeddedSessionId)) continue;
    ordered.push(attempt);
    selectedCandidateIds.add(attempt.embeddedSessionId);
  }
  for (const attempt of ordered) {
    if (attempt.classification === "missing_candidate") continue;
    const startedAtMs = attempt.startedAt ? Date.parse(attempt.startedAt) : null;
    if (startedAtMs !== null && attempt.frontendRecordingAtMs !== null) {
      attempt.wakeToVisibleMs = attempt.frontendRecordingAtMs - startedAtMs;
    }
    if (startedAtMs !== null && attempt.frontendRecordingAtMs !== null && attempt.wakeEndMs !== null) {
      attempt.phraseTailToVisibleMs = attempt.frontendRecordingAtMs - startedAtMs - attempt.wakeEndMs;
    }
    if (attempt.frontendRecordingAtMs !== null && attempt.silentExpiryAtMs !== null) {
      attempt.bodyWaitMs = attempt.silentExpiryAtMs - attempt.frontendRecordingAtMs;
    }

    if (attempt.decision === "pending") attempt.errors.push("missing terminal wake decision");
    if (attempt.pcmBytes === null) attempt.errors.push("missing BLE completion");
    if (attempt.missingPackets === null) attempt.errors.push("missing BLE packet result");
    else if (attempt.missingPackets !== 0) attempt.errors.push("BLE missing_packets is nonzero");

    if (attempt.decision === "rejected") {
      attempt.classification = "rejected";
      if (attempt.frontendRecordingAtMs !== null) attempt.errors.push("rejected candidate displayed Recording capsule");
      continue;
    }

    if (attempt.decision !== "accepted") continue;
    const hasBodyOutput = attempt.sideEffects.includes("text_or_clipboard_insertion")
      || attempt.sideEffects.includes("frontend_inserted_chars");
    const hasErrorSideEffect = attempt.sideEffects.includes("error_or_empty_transcript");
    attempt.classification = hasBodyOutput && !hasErrorSideEffect ? "body_present" : "wake_only";
    if (!attempt.coordinatorSessionId) attempt.errors.push("missing coordinator session link");
    if (attempt.backendRecordingAtMs === null) attempt.errors.push("missing backend visible Recording request");
    if (attempt.frontendRecordingAtMs === null) attempt.errors.push("frontend never received Recording capsule");
    if (attempt.classification === "wake_only" && attempt.silentExpiryAtMs === null) {
      attempt.errors.push("missing silent wake-only expiry");
    }
    if (attempt.classification === "wake_only" && attempt.wakeOnlyEventAtMs === null) {
      attempt.errors.push("missing wake_only_expired terminal event");
    }
    if (attempt.frontendIdleAtMs === null) attempt.errors.push("frontend did not return to Idle");
    if (attempt.classification === "wake_only" && attempt.bodyWaitMs !== null && attempt.bodyWaitMs < thresholds.bodyWaitMinMs) {
      attempt.errors.push(`wake-only body wait shorter than ${thresholds.bodyWaitMinMs} ms`);
    }
    if (attempt.wakeToVisibleMs !== null && attempt.wakeToVisibleMs > thresholds.wakeMaxMs) {
      attempt.errors.push(`wake-to-visible latency exceeds ${thresholds.wakeMaxMs} ms`);
    }
    if (attempt.phraseTailToVisibleMs !== null && attempt.phraseTailToVisibleMs > thresholds.phraseTailMaxMs) {
      attempt.errors.push(`phrase-tail-to-visible latency exceeds ${thresholds.phraseTailMaxMs} ms`);
    }
    if (attempt.classification === "wake_only") {
      for (const sideEffect of attempt.sideEffects) attempt.errors.push(`wake-only side effect: ${sideEffect}`);
    } else if (hasErrorSideEffect) {
      attempt.errors.push("wake-only side effect: error_or_empty_transcript");
    }
    attempt.success = attempt.classification === "wake_only" && attempt.errors.length === 0;
  }

  const successful = ordered.filter(attempt => attempt.success);
  const eligible = ordered.filter(attempt => attempt.classification !== "body_present");
  const acceptedVisible = ordered.filter(attempt => attempt.decision === "accepted" && attempt.frontendRecordingAtMs !== null);
  const wakeValues = acceptedVisible.map(attempt => attempt.wakeToVisibleMs).filter(Number.isFinite);
  const phraseTailValues = acceptedVisible.map(attempt => attempt.phraseTailToVisibleMs).filter(Number.isFinite);
  const aggregate = {
    wakeToVisibleP95Ms: percentile(wakeValues, 0.95),
    wakeToVisibleMaxMs: wakeValues.length > 0 ? Math.max(...wakeValues) : null,
    phraseTailToVisibleP95Ms: percentile(phraseTailValues, 0.95),
    phraseTailToVisibleMaxMs: phraseTailValues.length > 0 ? Math.max(...phraseTailValues) : null,
  };
  const failures = [];
  const missingLabeledAttemptIds = explicitAttemptIds.filter(id => !attempts.has(id));
  if (missingLabeledAttemptIds.length > 0) {
    failures.push(`missing explicitly labeled attempt id(s): ${missingLabeledAttemptIds.join(",")}`);
  }
  for (const attempt of ordered) {
    const label = attempt.embeddedSessionId === null
      ? `attempt_ordinal=${attempt.attemptOrdinal}`
      : `embedded_session_id=${attempt.embeddedSessionId}`;
    for (const error of attempt.errors) failures.push(`${label}: ${error}`);
  }
  if (transportFaults.length > 0) failures.push(`${transportFaults.length} QueueFull/notify/transport failure event(s) observed`);
  if (eligible.length >= thresholds.requiredAttempts && successful.length < thresholds.requiredSuccesses) {
    failures.push(`only ${successful.length}/${eligible.length} eligible attempts succeeded; ${thresholds.requiredSuccesses} required`);
  }
  if (eligible.length >= thresholds.requiredAttempts && aggregate.wakeToVisibleP95Ms > thresholds.wakeP95Ms) {
    failures.push(`wake-to-visible p95 exceeds ${thresholds.wakeP95Ms} ms`);
  }
  if (aggregate.wakeToVisibleMaxMs > thresholds.wakeMaxMs) {
    failures.push(`wake-to-visible max exceeds ${thresholds.wakeMaxMs} ms`);
  }
  if (eligible.length >= thresholds.requiredAttempts && aggregate.phraseTailToVisibleP95Ms > thresholds.phraseTailP95Ms) {
    failures.push(`phrase-tail-to-visible p95 exceeds ${thresholds.phraseTailP95Ms} ms`);
  }
  if (aggregate.phraseTailToVisibleMaxMs > thresholds.phraseTailMaxMs) {
    failures.push(`phrase-tail-to-visible max exceeds ${thresholds.phraseTailMaxMs} ms`);
  }

  const evidenceFailures = failures.length > 0;
  const status = !hasExplicitLabels
    ? "UNLABELED"
    : evidenceFailures
    ? "NO_GO"
    : eligible.length < thresholds.requiredAttempts
      ? "INCOMPLETE"
      : "PASS";
  return {
    schemaVersion: 2,
    scenario: "real-device-natural-wake-only",
    status,
    window: { after: options.after ?? null, before: options.before ?? null },
    thresholds,
    labeling: {
      required: true,
      explicitAttemptIds,
      missingLabeledAttemptIds,
      markerCount: attemptMarkers.length,
      missingCandidateOrdinals: ordered
        .filter(attempt => attempt.classification === "missing_candidate")
        .map(attempt => attempt.attemptOrdinal),
      unlabeledCandidateCount: observedCandidates.length - selectedCandidateIds.size,
    },
    observedCandidateCount: observedCandidates.length,
    observedAttemptCount: ordered.length,
    eligibleWakeOnlyAttemptCount: eligible.length,
    scenarioMismatchBodyCount: ordered.filter(attempt => attempt.classification === "body_present").length,
    successfulAttemptCount: successful.length,
    rejectedAttemptCount: ordered.filter(attempt => attempt.decision === "rejected").length,
    pendingAttemptCount: ordered.filter(attempt => attempt.decision === "pending").length,
    transportFaultCount: transportFaults.length,
    aggregate,
    failures,
    transportFaults,
    attempts: ordered,
  };
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  if (typeof args.attemptMarkers === "string") {
    const markerPayload = parseJsonFile(args.attemptMarkers);
    if (markerPayload?.schema !== "listener.live-wake-attempt-markers.v1" || !Array.isArray(markerPayload.attempts)) {
      failUsage("Invalid --attempt-markers payload");
    }
    args.attemptMarkers = markerPayload.attempts;
  }
  const combined = args.logs.map(path => readFileSync(path, "utf8")).join("\n");
  const report = analyzeLiveWakeOnlyLog(combined, args);
  const output = `${JSON.stringify(report, null, 2)}\n`;
  if (args.outputJson) {
    mkdirSync(dirname(resolve(args.outputJson)), { recursive: true });
    writeFileSync(args.outputJson, output, "utf8");
  }
  process.stdout.write(output);
  process.exitCode = report.status === "PASS" ? 0 : ["INCOMPLETE", "UNLABELED"].includes(report.status) ? 2 : 1;
}

if (resolve(process.argv[1] ?? "") === fileURLToPath(import.meta.url)) main();
