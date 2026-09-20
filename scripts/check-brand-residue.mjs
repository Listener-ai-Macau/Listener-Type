#!/usr/bin/env node
import { existsSync, readFileSync, readdirSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';
import process from 'node:process';

const root = process.cwd();
const blocked = [
  /OpenLess/,
  /openless/,
  /com\.openless/,
  /OpenLessIme/,
  /OPENLESS_/,
  /appergb\/openless/,
  /apic\.openless/,
  /openless\.top/,
  /openless\.app/,
  /\.openless/,
];
const ignoredDirs = new Set(['.artifacts', '.cache', '.git', '.omx', '.task', 'node_modules', 'target']);
const ignoredPrefixes = [
  'ref/',
  'src-tauri/target/',
  'docs/archive/upstream-provenance.md',
  'THIRD_PARTY_NOTICES.md',
  'scripts/check-brand-residue.mjs',
  'scripts/check-cloud-services.mjs',
  'scripts/check-doc-inheritance.mjs',
];
const textExts = new Set([
  '.c', '.cc', '.cmd', '.cpp', '.css', '.def', '.h', '.html', '.js', '.json',
  '.md', '.mjs', '.plist', '.ps1', '.py', '.rc', '.rs', '.sln', '.toml', '.ts',
  '.tsx', '.txt', '.vcxproj', '.wxs', '.xml', '.yml',
]);

function isAllowed(path) {
  return ignoredPrefixes.some(prefix => path === prefix.replace(/\/$/, '') || path.startsWith(prefix));
}

function extname(path) {
  const index = path.lastIndexOf('.');
  return index === -1 ? '' : path.slice(index);
}

function walk(dir, out = []) {
  for (const entry of readdirSync(dir)) {
    if (ignoredDirs.has(entry)) continue;
    const full = join(dir, entry);
    const rel = relative(root, full).replaceAll('\\', '/');
    if (isAllowed(rel)) continue;
    const st = statSync(full);
    if (st.isDirectory()) {
      walk(full, out);
    } else {
      out.push(rel);
    }
  }
  return out;
}

const failures = [];
for (const file of walk(root)) {
  for (const pattern of blocked) {
    if (pattern.test(file)) {
      failures.push(`${file}: filename matches ${pattern}`);
    }
  }
  if (!textExts.has(extname(file)) || !existsSync(file)) continue;
  const text = readFileSync(file, 'utf8');
  const lines = text.split(/\r?\n/);
  lines.forEach((line, index) => {
    for (const pattern of blocked) {
      if (pattern.test(line)) {
        failures.push(`${file}:${index + 1}: ${line.trim()}`);
      }
    }
  });
}

if (failures.length) {
  console.error('Brand residue audit failed:');
  console.error(failures.join('\n'));
  process.exit(1);
}

console.log('Brand residue audit passed.');
