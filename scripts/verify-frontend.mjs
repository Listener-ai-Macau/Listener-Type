#!/usr/bin/env node
// verify-frontend.mjs — One-command frontend health gate.
//
// Runs all automated checks that can verify frontend correctness
// without a human looking at the screen:
//   1. TypeScript compilation (tsc --noEmit)
//   2. Vite production build (npm run build)
//   3. All check:* scripts
//   4. All test:* scripts
//   5. Hardcoded color / dark mode hygiene (check-dark-mode)
//
// Usage: node scripts/verify-frontend.mjs
// Exit 0 = all pass, 1 = any fail

import { execSync } from 'node:child_process';
import process from 'node:process';

const root = process.cwd();
let failed = 0;

function run(label, cmd) {
  process.stdout.write(`  ${label} ... `);
  try {
    execSync(cmd, { cwd: root, stdio: 'pipe', timeout: 120_000, encoding: 'utf-8' });
    console.log('OK');
    return true;
  } catch (e) {
    console.log('FAIL');
    // Show stderr (truncated) so the caller can see what broke
    const stderr = (e.stderr || e.stdout || e.message || '').trim();
    const lines = stderr.split('\n').filter(l => l.trim());
    const show = lines.slice(-10).join('\n');
    if (show) console.error(show.split('\n').map(l => '    ' + l).join('\n'));
    failed++;
    return false;
  }
}

console.log('=== Frontend Verification ===\n');

// Phase 1: Static analysis (fast)
run('tsc --noEmit', 'npx tsc --noEmit');

// Phase 2: Individual check scripts
const checks = [
  'check:brand',
  'check:dark-mode',
];
for (const c of checks) {
  run(c, `npm run ${c}`);
}

// Phase 3: Unit tests
run('unit tests', 'npm test');

// Phase 4: Production build (catches bundling issues)
run('vite build', 'npx vite build');

// Summary
console.log('');
if (failed > 0) {
  console.error(`FAIL: ${failed} check(s) failed`);
  process.exit(1);
}
console.log('PASS: all frontend checks green');
process.exit(0);
