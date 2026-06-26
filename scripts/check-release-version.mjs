#!/usr/bin/env node
import { readFileSync } from 'node:fs';
import { spawnSync } from 'node:child_process';

function readJson(path) {
  return JSON.parse(readFileSync(path, 'utf8'));
}

function matchText(path, pattern, label) {
  const text = readFileSync(path, 'utf8');
  const match = text.match(pattern);
  if (!match) throw new Error(`Missing ${label} in ${path}`);
  return match[1];
}

const packageVersion = readJson('package.json').version;
const packageLock = readJson('package-lock.json');
const versions = new Map([
  ['package.json', packageVersion],
  ['package-lock.json', packageLock.version],
  ['package-lock root package', packageLock.packages?.['']?.version],
  ['src-tauri/tauri.conf.json', readJson('src-tauri/tauri.conf.json').version],
  ['src-tauri/Cargo.toml', matchText('src-tauri/Cargo.toml', /^version\s*=\s*"([^"]+)"/m, 'Cargo package version')],
  ['src-tauri/Cargo.lock listener-type', matchText('src-tauri/Cargo.lock', /name\s*=\s*"listener-type"\s*\nversion\s*=\s*"([^"]+)"/m, 'Cargo lock listener-type version')],
]);

const mismatches = [...versions].filter(([, version]) => version !== packageVersion);
if (mismatches.length > 0) {
  for (const [source, version] of versions) {
    console.error(`${source}: ${version}`);
  }
  throw new Error(`Listener Type version mismatch; expected every source to equal ${packageVersion}`);
}

if (!/^\d+\.\d+\.\d+$/.test(packageVersion)) {
  throw new Error(`Listener Type release version must be plain semver, got ${packageVersion}`);
}

if (process.argv.includes('--require-tag')) {
  const tag = `v${packageVersion}`;
  const result = spawnSync('git', ['describe', '--tags', '--exact-match'], { encoding: 'utf8' });
  if (result.status !== 0 || result.stdout.trim() !== tag) {
    throw new Error(`Current commit must be tagged ${tag}; got ${result.stdout.trim() || 'no exact tag'}`);
  }
}

console.log(`PASS: Listener Type release version is ${packageVersion}`);
