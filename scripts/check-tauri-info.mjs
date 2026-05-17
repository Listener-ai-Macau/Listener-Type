#!/usr/bin/env node
import { spawnSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const scriptsDir = dirname(fileURLToPath(import.meta.url));
const appRoot = join(scriptsDir, '..');
const tauriBin = join(
  appRoot,
  'node_modules',
  '.bin',
  process.platform === 'win32' ? 'tauri.cmd' : 'tauri',
);

const env = { ...process.env };
for (const key of ['HTTP_PROXY', 'HTTPS_PROXY', 'ALL_PROXY', 'http_proxy', 'https_proxy', 'all_proxy']) {
  delete env[key];
}

const result = spawnSync(tauriBin, ['info'], {
  cwd: appRoot,
  env,
  encoding: 'utf8',
  shell: process.platform === 'win32',
});

const output = `${result.stdout || ''}${result.stderr || ''}`;
if (result.status !== 0) {
  console.error(output);
  process.exit(result.status || 1);
}

for (const required of ['Environment', 'Packages', 'App']) {
  if (!output.includes(required)) {
    console.error(output);
    throw new Error(`tauri info output missing ${required}`);
  }
}

const packageJson = JSON.parse(readFileSync(join(appRoot, 'package.json'), 'utf8'));
const tauriConfig = JSON.parse(readFileSync(join(appRoot, 'src-tauri', 'tauri.conf.json'), 'utf8'));
if (packageJson.name !== 'listener-type') {
  throw new Error(`package.json name should be listener-type, got ${packageJson.name}`);
}
if (tauriConfig.productName !== 'Listener Type') {
  throw new Error(`tauri productName should be Listener Type, got ${tauriConfig.productName}`);
}
if (tauriConfig.identifier !== 'com.listener.type') {
  throw new Error(`tauri identifier should be com.listener.type, got ${tauriConfig.identifier}`);
}

console.log(output.trim());
