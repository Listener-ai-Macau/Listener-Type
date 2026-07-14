#!/usr/bin/env node
import { spawnSync } from 'node:child_process';
import process from 'node:process';

const checks = [
  ['dirty change inventory', 'npm run check:dirty-inventory'],
  ['regression guard commit index', 'npm run check:guard-commits'],
  ['repo hygiene', 'npm run check:repo-hygiene'],
  ['pwsh entrypoint hygiene', 'npm run check:pwsh-entrypoints'],
  ['version consistency', 'npm run check:version'],
  ['operator note triage gate', 'npm run check:preproduction-operator-notes'],
  ['preproduction bench contract', 'npm run check:preproduction-bench-contract'],
  ['recording low-latency regression contract', 'npm run check:recording-latency'],
  ['OTA speed log parser contract', 'npm run check:ota-speed-log-contract'],
  ['performance baseline contract', 'npm run check:performance-baselines'],
  ['frontend verification', 'npm run verify'],
  ['updater manifest generation', 'node scripts/write-updater-manifest.test.mjs'],
  [
    'tauri library tests',
    'cargo test --manifest-path src-tauri/Cargo.toml --lib -- --test-threads=1',
    { LISTENER_TYPE_DISABLE_BACKGROUND_BLE: '1' },
  ],
  ['firmware OTA headless helper tests', 'cargo test --manifest-path tools/firmware_ota_headless/Cargo.toml'],
];

function runCommand(commandLine, environment = {}) {
  const env = { ...process.env, ...environment };
  if (process.platform === 'win32') {
    return spawnSync(process.env.ComSpec || 'cmd.exe', ['/d', '/s', '/c', commandLine], {
      cwd: process.cwd(),
      env,
      stdio: 'inherit',
    });
  }
  return spawnSync('sh', ['-lc', commandLine], {
    cwd: process.cwd(),
    env,
    stdio: 'inherit',
  });
}

for (const [label, commandLine, environment] of checks) {
  console.log(`\n=== ${label} ===`);
  const result = runCommand(commandLine, environment);
  if (result.error) {
    console.error(result.error.message);
    process.exit(1);
  }
  if (result.status !== 0) {
    process.exit(result.status ?? 1);
  }
}

console.log('\nPASS: Listener Type release checks completed.');
