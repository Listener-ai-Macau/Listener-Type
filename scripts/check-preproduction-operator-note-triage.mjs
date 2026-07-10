import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

function argValue(name) {
  const index = process.argv.indexOf(name);
  if (index === -1) return "";
  return process.argv[index + 1] ?? "";
}

function readJson(filePath) {
  return JSON.parse(fs.readFileSync(filePath, "utf8"));
}

function sha256(text) {
  return crypto.createHash("sha256").update(text ?? "", "utf8").digest("hex");
}

function field(record, name) {
  if (!record || typeof record !== "object") return undefined;
  if (Object.prototype.hasOwnProperty.call(record, name)) return record[name];
  if (Array.isArray(record.Keys) && Array.isArray(record.Values)) {
    const index = record.Keys.indexOf(name);
    if (index >= 0) return record.Values[index];
  }
  if (record.SyncRoot && record.SyncRoot !== record) return field(record.SyncRoot, name);
  return undefined;
}

function noteText(record) {
  for (const name of ["operator_note", "operator_action", "observation"]) {
    const value = field(record, name);
    if (typeof value !== "string") continue;
    const text = value.trim();
    if (!text || text === "NoPrompt dry run") continue;
    return text;
  }
  return "";
}

function normalizedExistingPath(filePath) {
  if (!filePath || typeof filePath !== "string") return "";
  try {
    return fs.realpathSync.native(path.resolve(filePath));
  } catch {
    return "";
  }
}

function pathLooksSynthetic(filePath) {
  const normalized = String(filePath ?? "").toLowerCase();
  return [
    "operator-note",
    "dryrun",
    "dry-run",
    "smoke",
    "fixture",
    "script-gate",
    "format-check",
    "no-hardware",
  ].some((marker) => normalized.includes(marker));
}

function hasDryRunRecord(records) {
  return records.some((record) => {
    const dryRunNote = field(record, "dry_run_note");
    if (typeof dryRunNote === "string" && dryRunNote.trim() === "NoPrompt dry run") {
      return true;
    }
    return JSON.stringify(record ?? {}).includes("NoPrompt dry run");
  });
}

function isReleaseCandidateSummary(summaryPath) {
  if (pathLooksSynthetic(summaryPath)) return false;
  let summary;
  try {
    summary = readJson(summaryPath);
  } catch {
    return false;
  }
  const records = Array.isArray(summary.records) ? summary.records : [];
  if (records.length === 0 || hasDryRunRecord(records)) return false;

  const summaryDir = normalizedExistingPath(path.dirname(summaryPath));
  const outputDir = normalizedExistingPath(summary.output_dir);
  if (!summaryDir || outputDir !== summaryDir) return false;

  const sessionPath = normalizedExistingPath(summary.session_jsonl);
  if (!sessionPath || normalizedExistingPath(path.dirname(sessionPath)) !== summaryDir) {
    return false;
  }

  return true;
}

function latestSummary() {
  const roots = [
    path.join(repoRoot, ".cache", "validation"),
    path.join(repoRoot, ".artifacts", "v1.0.2-regression"),
  ].filter((root) => fs.existsSync(root));
  const matches = [];
  for (const root of roots) {
    const stack = [root];
    while (stack.length) {
      const current = stack.pop();
      for (const entry of fs.readdirSync(current, { withFileTypes: true })) {
        const full = path.join(current, entry.name);
        if (entry.isDirectory()) stack.push(full);
        if (entry.isFile() && entry.name === "preproduction-human-review-summary.json") {
          if (isReleaseCandidateSummary(full)) {
            matches.push(full);
          }
        }
      }
    }
  }
  matches.sort((a, b) => fs.statSync(b).mtimeMs - fs.statSync(a).mtimeMs);
  return matches[0] ?? "";
}

function collectStrings(value, output = []) {
  if (typeof value === "string") {
    output.push(value);
    return output;
  }
  if (Array.isArray(value)) {
    for (const item of value) collectStrings(item, output);
    return output;
  }
  if (value && typeof value === "object") {
    for (const item of Object.values(value)) collectStrings(item, output);
  }
  return output;
}

function resolveMaybeRepoPath(value) {
  if (!value || typeof value !== "string") return "";
  return path.isAbsolute(value) ? path.resolve(value) : path.resolve(repoRoot, value);
}

function trackedSummaryPaths() {
  const paths = new Set();
  const latest = latestSummary();
  if (latest) paths.add(path.resolve(latest));

  for (const rel of [
    ["scripts", "dirty-change-inventory.json"],
    ["scripts", "performance-baselines.json"],
  ]) {
    const filePath = path.join(repoRoot, ...rel);
    if (!fs.existsSync(filePath)) continue;
    const strings = collectStrings(readJson(filePath));
    for (const text of strings) {
      if (text.includes("preproduction-human-review-summary.json")) {
        paths.add(resolveMaybeRepoPath(text));
      }
    }
  }

  return [...paths].filter(Boolean);
}

function validateSummary(summaryPath, explicitTriagePath = "") {
  const failures = [];
  if (!summaryPath || !fs.existsSync(summaryPath)) {
    return { summaryPath, noteCount: 0, failures: [`preproduction human review summary not found: ${summaryPath}`] };
  }

  const summary = readJson(summaryPath);
  const records = Array.isArray(summary.records) ? summary.records : [];
  const recordNotes = records.map((record) => ({
    id: String(field(record, "id") ?? ""),
    title: String(field(record, "title") ?? ""),
    result: String(field(record, "result") ?? ""),
    operator_note: noteText(record),
  }));
  const notes = recordNotes.filter((record) => record.operator_note);

  if (records.length === 0) {
    failures.push(`${summaryPath}: human review summary has no records`);
  }

  if (notes.length === 0) {
    return { summaryPath, noteCount: 0, failures };
  }

  const triagePath = path.resolve(
    explicitTriagePath || path.join(path.dirname(summaryPath), "preproduction-operator-note-triage.json"),
  );
  if (!fs.existsSync(triagePath)) {
    failures.push(`${summaryPath}: operator note triage missing: ${triagePath}`);
    return { summaryPath, noteCount: notes.length, failures };
  }

  const triage = readJson(triagePath);
  const triageEntries = Array.isArray(triage.operator_notes) ? triage.operator_notes : [];
  const triageByKey = new Map();
  for (const entry of triageEntries) {
    const hash = String(entry.operator_note_sha256 ?? sha256(entry.operator_note ?? ""));
    triageByKey.set(`${entry.id}:${hash}`, entry);
  }

  if (triage.status !== "PASS") {
    failures.push(`${summaryPath}: triage status must be PASS, got ${triage.status ?? "missing"}`);
  }

  for (const note of notes) {
    const hash = sha256(note.operator_note);
    const entry = triageByKey.get(`${note.id}:${hash}`);
    if (!entry) {
      failures.push(`${summaryPath}: missing triage for ${note.id} note ${hash}`);
      continue;
    }

    const disposition = String(entry.disposition ?? "");
    if (!["fixed", "accepted_benign", "deferred_by_human"].includes(disposition)) {
      failures.push(`${summaryPath}: ${note.id} has invalid/open disposition '${disposition}'`);
    }

    if (entry.operator_note_acknowledged !== true) {
      failures.push(`${summaryPath}: ${note.id} must set operator_note_acknowledged=true after reading the full note as a prompt`);
    }

    const parsedRequests = Array.isArray(entry.parsed_requests)
      ? entry.parsed_requests.map((item) => String(item ?? "").trim()).filter(Boolean)
      : [];
    const triageNotes = String(entry.notes ?? "").trim();
    if (parsedRequests.length === 0 && triageNotes.length === 0) {
      failures.push(`${summaryPath}: ${note.id} triage must summarize the operator note or list parsed_requests`);
    }

    const evidence = Array.isArray(entry.evidence) ? entry.evidence.filter(Boolean) : [entry.evidence].filter(Boolean);
    if (evidence.length === 0) {
      failures.push(`${summaryPath}: ${note.id} triage must include evidence`);
    }
  }

  return { summaryPath, noteCount: notes.length, failures };
}

const explicitSummary = argValue("--summary");
const explicitTriage = argValue("--triage");
const summaryPaths = explicitSummary ? [path.resolve(explicitSummary)] : trackedSummaryPaths();
if (summaryPaths.length === 0) {
  console.error("FAIL: preproduction human review summary not found");
  process.exit(1);
}

const allFailures = [];
let summariesChecked = 0;
let notesChecked = 0;
for (const summaryPath of summaryPaths) {
  const result = validateSummary(summaryPath, explicitSummary ? explicitTriage : "");
  summariesChecked += 1;
  notesChecked += result.noteCount;
  allFailures.push(...result.failures);
}

if (allFailures.length) {
  console.error("FAIL: operator note triage is incomplete");
  for (const failure of allFailures) console.error(`- ${failure}`);
  process.exit(1);
}

console.log(`PASS: operator note triage covers ${notesChecked} non-empty human notes across ${summariesChecked} tracked human review summaries; blank notes are treated as normal pass.`);
