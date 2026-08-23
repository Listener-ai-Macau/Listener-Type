import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { spawnSync } from "node:child_process";

const root = resolve(import.meta.dirname, "..");
const fixtures = resolve(
  process.env.LISTENER_SPEAKER_EVALUATION_FIXTURES
    ?? resolve(root, ".artifacts", "speaker-evaluation", "public-corpus-v1", "fixtures"),
);
const output = resolve(process.argv[2] ?? resolve(root, ".artifacts", "target-speaker-overlap", "report.json"));
const work = resolve(dirname(output), "work");
const probe = resolve(
  process.env.LISTENER_VOLCENGINE_ASR_PROBE
    ?? resolve(
      root,
      "tools",
      "volcengine_asr_probe",
      "target",
      "debug",
      "listener-volcengine-asr-probe.exe",
    ),
);
const filterExe = resolve(
  process.env.LISTENER_TARGET_SPEAKER_FILTER_EXE
    ?? resolve(root, "src-tauri", "target", "release", "listener-type.exe"),
);

const owner = resolve(fixtures, "owner-clean-long-01.wav");
const nonOwner = resolve(fixtures, "non_owner-clean-long-01.wav");

for (const [label, path] of [["ASR probe", probe], ["target-speaker filter EXE", filterExe], ["owner fixture", owner], ["non-owner fixture", nonOwner]]) {
  if (!existsSync(path)) {
    throw new Error(`${label} is missing: ${path}`);
  }
}
const cases = Array.from({ length: 20 }, (_, index) => ({
  id: `overlap-${String(index + 1).padStart(2, "0")}`,
  nonOwnerDelayMs: [120, 220, 350, 500, 700, 900, 1100, 1350, 1550, 1800][index % 10],
  // Product acceptance must include an ordinary nearby conversation, not
  // only a background voice that is 6-15 dB quieter than the owner. The final
  // four cases deliberately make the interferer 3 dB louder so a PASS cannot
  // be manufactured by source-level imbalance.
  nonOwnerGainDb: [-12, -6, -3, 0, 3][Math.floor(index / 4) % 5],
  ownerGainDb: [0, -1, 1, 0][index % 4],
}));

function run(command, args, options = {}) {
  const result = spawnSync(command, args, {
    cwd: root,
    encoding: "utf8",
    windowsHide: true,
    ...options,
  });
  if (result.status !== 0) {
    throw new Error(`${command} failed (${result.status}): ${(result.stderr || result.stdout || "").trim()}`);
  }
  return result;
}

function normalize(value) {
  return value.normalize("NFKC").toLowerCase().replace(/[^\p{L}\p{N}]/gu, "");
}

function hash(value) {
  return createHash("sha256").update(value, "utf8").digest("hex");
}

function lcsLength(left, right) {
  const row = new Uint16Array(right.length + 1);
  for (let i = 1; i <= left.length; i += 1) {
    let diagonal = 0;
    for (let j = 1; j <= right.length; j += 1) {
      const prior = row[j];
      row[j] = left[i - 1] === right[j - 1]
        ? diagonal + 1
        : Math.max(row[j], row[j - 1]);
      diagonal = prior;
    }
  }
  return row[right.length];
}

function transcribe(audioPath, stem) {
  const jsonPath = resolve(work, `${stem}.json`);
  const result = spawnSync(probe, [
    "transcribe",
    "--audio", audioPath,
    "--pace-audio",
    "--timeout-seconds", "30",
    "--json-out", jsonPath,
  ], { cwd: root, encoding: "utf8", windowsHide: true, timeout: 45_000 });
  let report;
  try {
    report = JSON.parse(readFileSync(jsonPath, "utf8"));
  } catch {
    report = { status: "FAIL", error: (result.stderr || result.stdout || "probe produced no report").trim() };
  } finally {
    rmSync(jsonPath, { force: true });
  }
  if (result.error) report.error = result.error.message;
  return report;
}

mkdirSync(work, { recursive: true });

const ownerPcm = resolve(work, "owner.pcm");
run("ffmpeg", ["-hide_banner", "-loglevel", "error", "-y", "-i", owner, "-f", "s16le", "-ac", "1", "-ar", "16000", ownerPcm]);

const ownerBaseline = transcribe(owner, "owner-baseline");
const nonOwnerBaseline = transcribe(nonOwner, "non-owner-baseline");
if (ownerBaseline.status !== "PASS" || nonOwnerBaseline.status !== "PASS") {
  throw new Error(
    `Provider baseline transcription failed; overlap results would be invalid. `
      + `owner=${ownerBaseline.status}:${ownerBaseline.error ?? "unknown"} `
      + `nonOwner=${nonOwnerBaseline.status}:${nonOwnerBaseline.error ?? "unknown"}`,
  );
}

const expected = normalize(ownerBaseline.transcript ?? "");
const interferer = normalize(nonOwnerBaseline.transcript ?? "");
if (!expected || !interferer) throw new Error("Provider baseline transcript was empty.");
const targetCharacters = new Set([...expected]);
const interfererOnlyCharacters = new Set([...interferer].filter((character) => !targetCharacters.has(character)));
const results = [];

for (const testCase of cases) {
  const wav = resolve(work, `${testCase.id}.wav`);
  const mixPcm = resolve(work, `${testCase.id}.pcm`);
  const filteredPcm = resolve(work, `${testCase.id}-target.pcm`);
  const filteredWav = resolve(work, `${testCase.id}-target.wav`);
  const filter = [
    `[0:a]volume=${testCase.ownerGainDb}dB[owner]`,
    `[1:a]adelay=${testCase.nonOwnerDelayMs}|${testCase.nonOwnerDelayMs},volume=${testCase.nonOwnerGainDb}dB[room]`,
    "[owner][room]amix=inputs=2:duration=longest:dropout_transition=0,aresample=16000,pan=mono|c0=c0[out]",
  ].join(";");
  run("ffmpeg", ["-hide_banner", "-loglevel", "error", "-y", "-i", owner, "-i", nonOwner, "-filter_complex", filter, "-map", "[out]", "-ac", "1", "-ar", "16000", "-c:a", "pcm_s16le", wav]);
  run("ffmpeg", ["-hide_banner", "-loglevel", "error", "-y", "-i", wav, "-f", "s16le", "-ac", "1", "-ar", "16000", mixPcm]);
  const filterStarted = Date.now();
  run(filterExe, ["--diagnostic-target-speaker-filter", ownerPcm, mixPcm, filteredPcm], { timeout: 30_000 });
  const filterElapsedMs = Date.now() - filterStarted;
  run("ffmpeg", ["-hide_banner", "-loglevel", "error", "-y", "-f", "s16le", "-ar", "16000", "-ac", "1", "-i", filteredPcm, "-c:a", "pcm_s16le", filteredWav]);
  const provider = transcribe(filteredWav, testCase.id);
  const actual = normalize(provider.transcript ?? "");
  const targetRecall = expected.length === 0 ? 0 : lcsLength(expected, actual) / expected.length;
  const contaminationCharacters = [...actual].filter((character) => interfererOnlyCharacters.has(character)).length;
  const contamination = actual.length === 0 ? 0 : contaminationCharacters / actual.length;
  const passed = provider.status === "PASS" && targetRecall >= 0.95 && contamination <= 0.01;
  results.push({
    ...testCase,
    status: passed ? "PASS" : "FAIL",
    providerStatus: provider.status,
    targetRecall,
    contamination,
    actualCharacters: actual.length,
    filterElapsedMs,
    transcriptSha256: hash(actual),
    errorCategory: provider.error ? "provider_or_transport_error" : null,
  });
  for (const transient of [wav, mixPcm, filteredPcm, filteredWav, filteredPcm.replace(/\.pcm$/u, ".trace.txt")]) {
    rmSync(transient, { force: true });
  }
  process.stdout.write(`${testCase.id}: ${passed ? "PASS" : "FAIL"} recall=${targetRecall.toFixed(3)} contamination=${contamination.toFixed(3)}\n`);
}

const report = {
  schemaVersion: 1,
  generatedAt: new Date().toISOString(),
  corpus: "public-corpus-v1",
  providerMode: "local_enrolled_owner_filter_then_authoritative_bidirectional_paced",
  thresholds: { targetRecallMinimum: 0.95, contaminationMaximum: 0.01 },
  privacy: "No audio or transcript body is retained in this report; generated mixes and transient probe reports are deleted.",
  baseline: {
    ownerNormalizedCharacters: expected.length,
    ownerTranscriptSha256: hash(expected),
    nonOwnerNormalizedCharacters: interferer.length,
    nonOwnerTranscriptSha256: hash(interferer),
  },
  summary: {
    total: results.length,
    passed: results.filter((item) => item.status === "PASS").length,
    failed: results.filter((item) => item.status !== "PASS").length,
    minimumTargetRecall: Math.min(...results.map((item) => item.targetRecall)),
    maximumContamination: Math.max(...results.map((item) => item.contamination)),
  },
  results,
};
report.status = report.summary.failed === 0 ? "PASS" : "FAIL";
mkdirSync(dirname(output), { recursive: true });
writeFileSync(output, `${JSON.stringify(report, null, 2)}\n`, "utf8");
rmSync(work, { recursive: true, force: true });
process.stdout.write(`report=${output}\nstatus=${report.status}\n`);
process.exitCode = report.status === "PASS" ? 0 : 1;
