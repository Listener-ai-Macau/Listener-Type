import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve, dirname, basename } from 'node:path';
import { spawnSync } from 'node:child_process';
const root = resolve(import.meta.dirname, '..');
const original = JSON.parse(readFileSync(join(root, 'scripts/regression-guard-commits.json'), 'utf8'));
const base = original.entries[0];
function check(mutate, { scopeRepo = base.repo, absentRepository = false, publicHistory = false } = {}) {
  const directory = mkdtempSync(join(tmpdir(), 'listener-guard-contract-'));
  assert.equal(dirname(resolve(directory)), resolve(tmpdir()));
  assert.ok(basename(directory).startsWith('listener-guard-contract-'));
  try {
    const value = { schema_version: 1, repositories: { [base.repo]: structuredClone(original.repositories[base.repo]) }, entries: [structuredClone(base)], required_guards: [] };
    if (absentRepository) value.repositories.absent = { path: join(directory, 'no-checkout') };
    mutate(value.entries[0]);
    const path = join(directory, 'manifest.json');
    writeFileSync(path, JSON.stringify(value));
    const args = [join(root, 'scripts/check-regression-guard-commits.mjs'), '--manifest', path];
    if (scopeRepo) args.push('--repo', scopeRepo);
    if (publicHistory) args.push('--history', 'public');
    const result = spawnSync(process.execPath, args, { encoding: 'utf8' });
    return { code: result.status, output: result.stdout + result.stderr };
  } finally { rmSync(directory, { recursive: true }); }
}
test('feature description is distinct from an exact existing commit subject', () => {
  const result = check(entry => { entry.commit_subject = entry.subject; entry.subject = 'Feature behavior description'; });
  assert.equal(result.code, 0, result.output);
});
test('a wrong exact commit subject is rejected', () => {
  const result = check(entry => { entry.commit_subject = 'Wrong commit title'; });
  assert.notEqual(result.code, 0); assert.match(result.output, /subject mismatch/);
});
test('a missing commit is rejected', () => {
  const result = check(entry => { entry.commit = '0'.repeat(40); });
  assert.notEqual(result.code, 0); assert.match(result.output, /cat-file/);
});
test('an accepted feature cannot silently lose its executable guard', () => {
  const result = check(entry => { entry.guards = []; });
  assert.notEqual(result.code, 0); assert.match(result.output, /guards/);
});
test('an evidence-only failed attempt is retained without becoming acceptance', () => {
  const result = check(entry => { entry.guards = []; entry.validation_kind = 'evidence_only'; entry.humanAccepted = false; entry.state = 'physical_pair_no_gain_rolled_back'; entry.evidence = ['retained/failure.json']; });
  assert.equal(result.code, 0, result.output); assert.match(result.output, /1 evidence-only/);
});
test('evidence-only records cannot promote themselves to human acceptance', () => {
  const result = check(entry => { entry.guards = []; entry.validation_kind = 'evidence_only'; entry.humanAccepted = true; entry.evidence = ['retained/failure.json']; });
  assert.notEqual(result.code, 0); assert.match(result.output, /cannot claim human acceptance/);
});
test('evidence-only records must retain failure evidence references', () => {
  const result = check(entry => { entry.guards = []; entry.validation_kind = 'evidence_only'; entry.humanAccepted = false; entry.state = 'physical_pair_no_gain_rolled_back'; entry.evidence = []; });
  assert.notEqual(result.code, 0); assert.match(result.output, /evidence/);
});

test('a single-repository checkout can verify its commits without an unrelated checkout', () => {
  const result = check(() => {}, { absentRepository: true });
  assert.equal(result.code, 0, result.output);
  assert.match(result.output, /Other repositories retain structural checks only/);
});

test('default full verification still rejects a missing repository', () => {
  const result = check(() => {}, { scopeRepo: null, absentRepository: true });
  assert.notEqual(result.code, 0);
  assert.match(result.output, /repository absent path does not exist/);
});

function retainedLocalSnapshot(entry) {
  entry.commit = '0'.repeat(40);
  entry.commit_availability = 'local_snapshot';
  entry.local_retention_ref = `refs/tags/regression-baseline/type-${entry.commit.slice(0, 12)}`;
  entry.public_history_exclusion_reason = 'Historical binary-bearing snapshot retained on the original checkout';
}

test('public history reports retained local snapshots as unverified, not existing public commits', () => {
  const result = check(retainedLocalSnapshot, { publicHistory: true });
  assert.equal(result.code, 0, result.output);
  assert.match(result.output, /0 existing commits verified/);
  assert.match(result.output, /1 retained declarations, Git objects NOT verified/);
});

test('complete history still rejects a missing local snapshot commit', () => {
  const result = check(retainedLocalSnapshot);
  assert.notEqual(result.code, 0);
  assert.match(result.output, /cat-file/);
});

test('public local-snapshot declarations require an exact fixed retention ref', () => {
  const result = check(entry => { retainedLocalSnapshot(entry); delete entry.local_retention_ref; }, { publicHistory: true });
  assert.notEqual(result.code, 0);
  assert.match(result.output, /exact fixed local tag/);
});
