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
          matches.push(full);
        }
      }
    }
  }
  matches.sort((a, b) => fs.statSync(b).mtimeMs - fs.statSync(a).mtimeMs);
  return matches[0] ?? "";
}

const summaryPath = path.resolve(argValue("--summary") || latestSummary());
if (!summaryPath || !fs.existsSync(summaryPath)) {
  console.error("FAIL: preproduction human review summary not found");
  process.exit(1);
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

const failures = [];
if (records.length === 0) {
  failures.push("human review summary has no records");
}

if (notes.length === 0) {
  if (failures.length) {
    console.error("FAIL: operator note triage is incomplete");
    for (const failure of failures) console.error(`- ${failure}`);
    process.exit(1);
  }
  console.log(`PASS: operator note triage has no non-empty human notes in ${summaryPath}; blank notes are treated as normal pass.`);
  process.exit(0);
}

const triagePath = path.resolve(
  argValue("--triage") || path.join(path.dirname(summaryPath), "preproduction-operator-note-triage.json"),
);
if (!fs.existsSync(triagePath)) {
  console.error(`FAIL: operator note triage missing: ${triagePath}`);
  process.exit(1);
}

const triage = readJson(triagePath);

const triageEntries = Array.isArray(triage.operator_notes) ? triage.operator_notes : [];
const triageByKey = new Map();
for (const entry of triageEntries) {
  const hash = String(entry.operator_note_sha256 ?? sha256(entry.operator_note ?? ""));
  triageByKey.set(`${entry.id}:${hash}`, entry);
}

if (triage.status !== "PASS") {
  failures.push(`triage status must be PASS, got ${triage.status ?? "missing"}`);
}

for (const note of notes) {
  const hash = sha256(note.operator_note);
  const entry = triageByKey.get(`${note.id}:${hash}`);
  if (!entry) {
    failures.push(`missing triage for ${note.id} note ${hash}`);
    continue;
  }

  const disposition = String(entry.disposition ?? "");
  if (!["fixed", "accepted_benign", "deferred_by_human"].includes(disposition)) {
    failures.push(`${note.id} has invalid/open disposition '${disposition}'`);
  }

  if (entry.operator_note_acknowledged !== true) {
    failures.push(`${note.id} must set operator_note_acknowledged=true after reading the full note as a prompt`);
  }

  const parsedRequests = Array.isArray(entry.parsed_requests)
    ? entry.parsed_requests.map((item) => String(item ?? "").trim()).filter(Boolean)
    : [];
  const triageNotes = String(entry.notes ?? "").trim();
  if (parsedRequests.length === 0 && triageNotes.length === 0) {
    failures.push(`${note.id} triage must summarize the operator note or list parsed_requests`);
  }

  const evidence = Array.isArray(entry.evidence) ? entry.evidence.filter(Boolean) : [entry.evidence].filter(Boolean);
  if (evidence.length === 0) {
    failures.push(`${note.id} triage must include evidence`);
  }
}

if (failures.length) {
  console.error("FAIL: operator note triage is incomplete");
  for (const failure of failures) console.error(`- ${failure}`);
  process.exit(1);
}

console.log(`PASS: operator note triage covers ${notes.length} non-empty human notes from ${summaryPath}; blank notes are treated as normal pass.`);
