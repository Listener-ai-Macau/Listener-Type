#!/usr/bin/env node
import { existsSync, readFileSync } from 'node:fs';
import { dirname, join, normalize, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
import process from 'node:process';

const scriptDir = dirname(fileURLToPath(import.meta.url));
const typeRoot = normalize(join(scriptDir, '..'));
const listenerRoot = normalize(join(typeRoot, '..'));
const manifestPath = join(scriptDir, 'regression-guard-commits.json');

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
for (const [id, repo] of Object.entries(manifest.repositories)) {
  requireString(repo.path, `repositories.${id}.path`);
  const repoRoot = resolve(listenerRoot, repo.path);
  if (!existsSync(repoRoot)) fail(`repository ${id} path does not exist: ${repoRoot}`);
  repoRoots.set(id, repoRoot);
}

const seenIds = new Set();
const seenGuardKeys = new Set();
for (const [index, entry] of manifest.entries.entries()) {
  for (const field of ['id', 'repo', 'commit', 'subject', 'state']) {
    requireString(entry[field], `entries[${index}].${field}`);
  }
  requireStringArray(entry.protects, `entries[${index}].protects`);
  requireStringArray(entry.guards, `entries[${index}].guards`);
  if (seenIds.has(entry.id)) fail(`duplicate regression guard entry id: ${entry.id}`);
  seenIds.add(entry.id);
  if (!/^[0-9a-f]{40}$/i.test(entry.commit)) {
    fail(`entry ${entry.id} commit must be a full 40-character SHA: ${entry.commit}`);
  }
  const repoRoot = repoRoots.get(entry.repo);
  if (!repoRoot) fail(`entry ${entry.id} references unknown repo: ${entry.repo}`);
  runGit(repoRoot, ['cat-file', '-e', `${entry.commit}^{commit}`]);
  const actualSubject = runGit(repoRoot, ['show', '-s', '--format=%s', entry.commit]);
  if (actualSubject !== entry.subject) {
    fail(`entry ${entry.id} subject mismatch for ${entry.commit}: expected '${entry.subject}', got '${actualSubject}'`);
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

console.log(`PASS: ${manifest.entries.length} regression guard entries map to existing Type/Firmware commits.`);
for (const entry of manifest.entries) {
  console.log(`- ${entry.repo} ${entry.commit.slice(0, 7)} ${entry.id}: ${entry.subject}`);
}
