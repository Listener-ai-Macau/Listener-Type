#!/usr/bin/env node
import { existsSync, readFileSync } from 'node:fs';
import { dirname, join, normalize } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
import process from 'node:process';

const scriptDir = dirname(fileURLToPath(import.meta.url));
const typeRoot = normalize(join(scriptDir, '..'));
const firmwareRoot = normalize(join(typeRoot, '..', 'Listener-Firmware'));
const inventoryPath = join(scriptDir, 'dirty-change-inventory.json');

function fail(message) {
  throw new Error(message);
}

function gitStatus(repoRoot) {
  const result = spawnSync('git', ['-C', repoRoot, 'status', '--porcelain=v1', '--untracked-files=all'], {
    encoding: 'utf8',
  });
  if (result.error) fail(result.error.message);
  if (result.status !== 0) {
    fail(`git status failed for ${repoRoot}: ${(result.stderr || result.stdout || '').trim()}`);
  }
  return result.stdout
    .replace(/\r\n/g, '\n')
    .split('\n')
    .map(line => line.trimEnd())
    .filter(Boolean)
    .map(line => {
      const rawPath = line.slice(3).trim();
      const path = rawPath.includes(' -> ') ? rawPath.split(' -> ').pop() : rawPath;
      return {
        status: line.slice(0, 2),
        path: path.replace(/\\/g, '/'),
      };
    });
}

function globToRegex(pattern) {
  let out = '^';
  for (let i = 0; i < pattern.length; i += 1) {
    const ch = pattern[i];
    const next = pattern[i + 1];
    if (ch === '*' && next === '*') {
      const after = pattern[i + 2];
      if (after === '/') {
        out += '(?:.*/)?';
        i += 2;
      } else {
        out += '.*';
        i += 1;
      }
      continue;
    }
    if (ch === '*') {
      out += '[^/]*';
      continue;
    }
    if ('\\^$+?.()|{}[]'.includes(ch)) {
      out += `\\${ch}`;
      continue;
    }
    out += ch;
  }
  out += '$';
  return new RegExp(out);
}

function validateEntry(entry, index) {
  for (const field of ['id', 'repo', 'item', 'state', 'patterns', 'evidence']) {
    if (!(field in entry)) fail(`inventory entry ${index} missing '${field}'`);
  }
  if (!['type', 'firmware', 'both'].includes(entry.repo)) {
    fail(`inventory entry ${entry.id} has invalid repo '${entry.repo}'`);
  }
  if (!Array.isArray(entry.patterns) || entry.patterns.length === 0) {
    fail(`inventory entry ${entry.id} must contain at least one pattern`);
  }
  if (!Array.isArray(entry.evidence) || entry.evidence.length === 0) {
    fail(`inventory entry ${entry.id} must contain at least one evidence item`);
  }
}

if (!existsSync(inventoryPath)) {
  fail(`dirty change inventory missing: ${inventoryPath}`);
}

const inventory = JSON.parse(readFileSync(inventoryPath, 'utf8'));
if (inventory.schema_version !== 1) fail('dirty change inventory schema_version must be 1');
if (!Array.isArray(inventory.entries)) fail('dirty change inventory entries must be an array');
inventory.entries.forEach(validateEntry);

const compiledEntries = inventory.entries.map(entry => ({
  ...entry,
  regexes: entry.patterns.map(globToRegex),
}));

const dirty = [
  ...gitStatus(typeRoot).map(item => ({ repo: 'type', ...item })),
  ...gitStatus(firmwareRoot).map(item => ({ repo: 'firmware', ...item })),
];

const uncovered = [];
const grouped = new Map();
for (const item of dirty) {
  const matches = compiledEntries.filter(entry =>
    (entry.repo === item.repo || entry.repo === 'both') &&
    entry.regexes.some(regex => regex.test(item.path)),
  );
  if (matches.length === 0) {
    uncovered.push(item);
    continue;
  }
  const key = matches[0].id;
  if (!grouped.has(key)) {
    grouped.set(key, { entry: matches[0], paths: [] });
  }
  grouped.get(key).paths.push(item);
}

if (uncovered.length > 0) {
  console.error('FAIL: dirty worktree contains unclassified paths.');
  for (const item of uncovered) {
    console.error(`${item.repo}: ${item.status} ${item.path}`);
  }
  process.exit(1);
}

console.log(`PASS: all ${dirty.length} dirty paths are classified in scripts/dirty-change-inventory.json.`);
for (const { entry, paths } of grouped.values()) {
  console.log(`- ${entry.id}: ${paths.length} path(s), state=${entry.state}, item=${entry.item}`);
}
