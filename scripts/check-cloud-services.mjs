#!/usr/bin/env node
import { readFileSync } from 'node:fs';
import process from 'node:process';

const files = [
  'src-tauri/tauri.conf.json',
  'src-tauri/src/commands.rs',
  'src/lib/ipc.ts',
  'scripts/write-updater-manifest.mjs',
];

const forbidden = [
  /appergb\/openless/,
  /apic\.openless/,
  /openless\.top/,
  /openless\.app/,
  /fastgit/,
  /apic\.listener-type/,
  /api\.listener-type/,
  /listener-type\.top/,
  /listener-type\.app/,
  /Ov23liyv3nEucG7oMHNE/,
];

const failures = [];
for (const file of files) {
  const text = readFileSync(file, 'utf8');
  const lines = text.split(/\r?\n/);
  lines.forEach((line, index) => {
    for (const pattern of forbidden) {
      if (pattern.test(line)) {
        failures.push(`${file}:${index + 1}: ${line.trim()}`);
      }
    }
  });
}

const tauri = JSON.parse(readFileSync('src-tauri/tauri.conf.json', 'utf8'));
const endpoints = tauri.plugins?.updater?.endpoints ?? [];
for (const endpoint of endpoints) {
  if (!endpoint.includes('github.com/Listener-ai-Macau/Listener-Type/')) {
    failures.push(`src-tauri/tauri.conf.json: updater endpoint is not Listener Type owned: ${endpoint}`);
  }
}

const commands = readFileSync('src-tauri/src/commands.rs', 'utf8');
if (!/const GITHUB_OAUTH_CLIENT_ID:\s*&str\s*=\s*"";/.test(commands)) {
  failures.push('src-tauri/src/commands.rs: GITHUB_OAUTH_CLIENT_ID must be empty by default.');
}
if (!/LISTENER_TYPE_MARKETPLACE_BASE_URL/.test(commands)) {
  failures.push('src-tauri/src/commands.rs: marketplace must be configurable through LISTENER_TYPE_MARKETPLACE_BASE_URL.');
}

const ipc = readFileSync('src/lib/ipc.ts', 'utf8');
if (!/marketplaceBaseUrl:\s*''/.test(ipc)) {
  failures.push('src/lib/ipc.ts: marketplaceBaseUrl default must be blank.');
}

if (failures.length) {
  console.error('Cloud service audit failed:');
  console.error(failures.join('\n'));
  process.exit(1);
}

console.log('Cloud service audit passed.');
