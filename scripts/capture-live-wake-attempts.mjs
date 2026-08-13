#!/usr/bin/env node
import { mkdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import process from "node:process";
import { createInterface } from "node:readline/promises";
import { fileURLToPath } from "node:url";

const DEFAULT_ATTEMPTS = 5;
const DEFAULT_ASSOCIATION_TIMEOUT_MS = 6_000;
const POLL_MS = 100;

function usage(message) {
  console.error(
    `${message}\n\nUsage: node scripts/capture-live-wake-attempts.mjs --log <listener-type.log> ` +
      "--output-json <attempt-markers.json> [--attempts 5] [--association-timeout-ms 6000]",
  );
  process.exit(2);
}

function parseArgs(argv) {
  const parsed = {
    attempts: DEFAULT_ATTEMPTS,
    associationTimeoutMs: DEFAULT_ASSOCIATION_TIMEOUT_MS,
  };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    const value = argv[index + 1];
    if (!arg.startsWith("--") || value === undefined || value.startsWith("--")) {
      usage(`Invalid argument: ${arg}`);
    }
    index += 1;
    const key = arg.slice(2).replace(/-([a-z])/g, (_, ch) => ch.toUpperCase());
    parsed[key] = ["attempts", "associationTimeoutMs"].includes(key)
      ? Number.parseInt(value, 10)
      : value;
  }
  if (!parsed.log) usage("Missing --log");
  if (!parsed.outputJson) usage("Missing --output-json");
  if (!Number.isInteger(parsed.attempts) || parsed.attempts < 1 || parsed.attempts > 100) {
    usage("--attempts must be an integer from 1 to 100");
  }
  if (!Number.isInteger(parsed.associationTimeoutMs) || parsed.associationTimeoutMs < 1_000) {
    usage("--association-timeout-ms must be at least 1000");
  }
  return parsed;
}

export function voiceActivationStarts(text) {
  const starts = [];
  for (const line of text.split(/\r?\n/)) {
    const match = /event=start embedded_session_id=(\d+) origin=VoiceActivation/.exec(line);
    if (!match) continue;
    const embeddedSessionId = Number.parseInt(match[1], 10);
    if (!starts.some(item => item.embeddedSessionId === embeddedSessionId)) {
      starts.push({ embeddedSessionId, line });
    }
  }
  return starts;
}

export function makeMarkerReport({ log, associationTimeoutMs, attempts, startedAt, completedAt }) {
  return {
    schema: "listener.live-wake-attempt-markers.v1",
    scenario: "real-device-natural-wake-only",
    log: resolve(log),
    startedAt,
    completedAt,
    associationTimeoutMs,
    attempts,
  };
}

function sleep(ms) {
  return new Promise(resolvePromise => setTimeout(resolvePromise, ms));
}

async function waitForCandidate(logPath, startOffset, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const text = readFileSync(logPath, "utf8");
    const appended = Buffer.from(text, "utf8").subarray(startOffset).toString("utf8");
    const [candidate] = voiceActivationStarts(appended);
    if (candidate) return candidate.embeddedSessionId;
    await sleep(POLL_MS);
  }
  return null;
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const logPath = resolve(args.log);
  const outputPath = resolve(args.outputJson);
  const prompt = createInterface({ input: process.stdin, output: process.stdout });
  const attempts = [];
  const startedAt = new Date().toISOString();

  console.log("Listener 真人唤醒标注：不会保存音频或识别文本。");
  console.log("每轮按 Enter 后立即自然说一次“开始录音”；上一轮胶囊结束并再等约 3 秒后，才开始下一轮。");

  try {
    for (let ordinal = 1; ordinal <= args.attempts; ordinal += 1) {
      await prompt.question(`\n第 ${ordinal}/${args.attempts} 轮准备好后按 Enter：`);
      const markedAt = new Date().toISOString();
      const logByteOffset = statSync(logPath).size;
      console.log("现在说：开始录音");
      const embeddedSessionId = await waitForCandidate(
        logPath,
        logByteOffset,
        args.associationTimeoutMs,
      );
      const associationEndedAt = new Date().toISOString();
      attempts.push({
        ordinal,
        markedAt,
        associationEndedAt,
        logByteOffset,
        embeddedSessionId,
      });
      console.log(
        embeddedSessionId === null
          ? `第 ${ordinal} 轮：未观察到设备候选（将明确计为失败）`
          : `第 ${ordinal} 轮：已关联 embedded_session_id=${embeddedSessionId}`,
      );
    }
  } finally {
    prompt.close();
  }

  const report = makeMarkerReport({
    log: logPath,
    associationTimeoutMs: args.associationTimeoutMs,
    attempts,
    startedAt,
    completedAt: new Date().toISOString(),
  });
  mkdirSync(dirname(outputPath), { recursive: true });
  writeFileSync(outputPath, `${JSON.stringify(report, null, 2)}\n`, "utf8");
  console.log(`\n标注完成：${outputPath}`);
  console.log(`关联候选 ${attempts.filter(item => item.embeddedSessionId !== null).length}/${attempts.length}`);
}

if (resolve(process.argv[1] ?? "") === fileURLToPath(import.meta.url)) {
  main().catch(error => {
    console.error(error instanceof Error ? error.stack : String(error));
    process.exitCode = 1;
  });
}
