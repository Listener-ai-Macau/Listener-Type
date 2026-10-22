#!/usr/bin/env node
import { existsSync, readFileSync } from 'node:fs';
import { dirname, join, normalize, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
import process from 'node:process';

const scriptDir = dirname(fileURLToPath(import.meta.url));
const typeRoot = normalize(join(scriptDir, '..'));
const listenerRoot = normalize(join(typeRoot, '..'));
const args = process.argv.slice(2);
const options = new Map();
for (let index = 0; index < args.length; index += 2) {
  const option = args[index], value = args[index + 1];
  if (!['--manifest', '--repo'].includes(option) || !value || value.startsWith('--') || options.has(option)) {
    throw new Error('Usage: check-regression-guard-commits.mjs [--manifest path] [--repo repository-id]');
  }
  options.set(option, value);
}
const manifestPath = options.has('--manifest') ? resolve(options.get('--manifest')) : join(scriptDir, 'regression-guard-commits.json');
const selectedRepo = options.get('--repo');

function fail(message) {
  throw new Error(message);
}

function runGit(repoRoot, args) {
  const result = spawnSync('git', ['-C', repoRoot, ...args], {
    encoding: 'utf8',
  });
  if (result.error) fail(result.error.message);
  if (result.status !== 0) {
    fail(`git -C ${repoRoot} ${args.join(' ')} failed: ${(result.stderr || result.stdout || '').trim()}`);
  }
  return result.stdout.trim();
}

function requireString(value, label) {
  if (typeof value !== 'string' || value.trim().length === 0) {
    fail(`${label} must be a non-empty string`);
  }
}

function requireStringArray(value, label) {
  if (!Array.isArray(value) || value.length === 0) {
    fail(`${label} must be a non-empty array`);
  }
  value.forEach((item, index) => requireString(item, `${label}[${index}]`));
}

if (!existsSync(manifestPath)) {
  fail(`regression guard commit manifest missing: ${manifestPath}`);
}

const manifest = JSON.parse(readFileSync(manifestPath, 'utf8'));
if (manifest.schema_version !== 1) fail('regression guard commit manifest schema_version must be 1');
if (!manifest.repositories || typeof manifest.repositories !== 'object') {
  fail('regression guard commit manifest must define repositories');
}
if (!Array.isArray(manifest.entries) || manifest.entries.length === 0) {
  fail('regression guard commit manifest entries must be a non-empty array');
}

const repoRoots = new Map();
if (selectedRepo && !Object.hasOwn(manifest.repositories, selectedRepo)) fail(`unknown selected repository: ${selectedRepo}`);
for (const [id, repo] of Object.entries(manifest.repositories)) {
  requireString(repo.path, `repositories.${id}.path`);
  if (selectedRepo && id !== selectedRepo) continue;
  const repoRoot = resolve(listenerRoot, repo.path);
  if (!existsSync(repoRoot)) fail(`repository ${id} path does not exist: ${repoRoot}`);
  repoRoots.set(id, repoRoot);
}

const seenIds = new Set();
const seenGuardKeys = new Set();
let verifiedCommits = 0;
for (const [index, entry] of manifest.entries.entries()) {
  for (const field of ['id', 'repo', 'commit', 'subject', 'state']) {
    requireString(entry[field], `entries[${index}].${field}`);
  }
  requireStringArray(entry.protects, `entries[${index}].protects`);
  if (entry.validation_kind === 'evidence_only') {
    if (!Array.isArray(entry.guards) || entry.guards.length !== 0) fail(`evidence-only ${entry.id} must not masquerade as executable guards`);
    if (entry.humanAccepted !== false || (/human_accepted/.test(entry.state) && !/not_human_accepted/.test(entry.state))) {
      fail(`evidence-only ${entry.id} cannot claim human acceptance`);
    }
    requireStringArray(entry.evidence, `entries[${index}].evidence`);
  } else {
    requireStringArray(entry.guards, `entries[${index}].guards`);
  }
  if (seenIds.has(entry.id)) fail(`duplicate regression guard entry id: ${entry.id}`);
  seenIds.add(entry.id);
  if (!/^[0-9a-f]{40}$/i.test(entry.commit)) {
    fail(`entry ${entry.id} commit must be a full 40-character SHA: ${entry.commit}`);
  }
  if (!Object.hasOwn(manifest.repositories, entry.repo)) fail(`entry ${entry.id} references unknown repo: ${entry.repo}`);
  const expectedCommitSubject = entry.commit_subject ?? entry.subject;
  requireString(expectedCommitSubject, `entries[${index}].commit_subject`);
  const repoRoot = repoRoots.get(entry.repo);
  if (repoRoot) {
    runGit(repoRoot, ['cat-file', '-e', `${entry.commit}^{commit}`]);
    const actualSubject = runGit(repoRoot, ['show', '-s', '--format=%s', entry.commit]);
    if (actualSubject !== expectedCommitSubject) {
      fail(`entry ${entry.id} subject mismatch for ${entry.commit}: expected '${expectedCommitSubject}', got '${actualSubject}'`);
    }
    verifiedCommits += 1;
  }
  for (const guard of entry.guards) {
    const guardKey = `${entry.repo}:${guard}`;
    seenGuardKeys.add(guardKey);
  }
}

for (const requiredGuard of manifest.required_guards ?? []) {
  requireString(requiredGuard.repo, 'required_guards.repo');
  requireString(requiredGuard.guard, 'required_guards.guard');
  const key = `${requiredGuard.repo}:${requiredGuard.guard}`;
  if (!seenGuardKeys.has(key)) {
    fail(`required guard is not linked to any commit: ${key}`);
  }
}

const evidenceOnly = manifest.entries.filter(entry => entry.validation_kind === 'evidence_only').length;
console.log(`PASS: ${manifest.entries.length} history entries structurally checked; ${verifiedCommits} existing commits verified (scope: ${selectedRepo ?? 'all repositories'}; ${evidenceOnly} evidence-only; this is not a current-runtime feature acceptance).`);
if (selectedRepo) console.log('Other repositories retain structural checks only; their Git objects were not verified in this checkout.');
for (const entry of manifest.entries) {
  console.log(`- ${entry.repo} ${entry.commit.slice(0, 7)} ${entry.id}: ${entry.subject}`);
}
