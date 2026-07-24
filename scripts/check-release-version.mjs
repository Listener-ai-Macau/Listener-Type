#!/usr/bin/env node
import { spawnSync } from 'node:child_process';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const scriptDir = dirname(fileURLToPath(import.meta.url));
const appRoot = join(scriptDir, '..');
const toolPath = join(appRoot, 'third_party', 'denzic-platform', 'tools', 'release_gate', 'version_check.py');
const configPath = join(scriptDir, 'release-version-gate.json');

const args = [toolPath, '--config', configPath];
if (process.argv.includes('--require-tag')) {
  args.push('--require-tag');
}

const result = spawnSync('python', args, { stdio: 'inherit' });
if (result.error) {
  console.error(result.error.message);
  process.exit(1);
}
process.exit(result.status ?? 1);
