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
import { readFileSync, readdirSync } from 'node:fs';
import { join, relative } from 'node:path';
import process from 'node:process';

const root = process.cwd();
let failed = 0;

function walkFiles(dir, cb) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    if (entry.name.startsWith('.') || entry.name === 'node_modules' || entry.name === 'target') continue;
    const fullPath = join(dir, entry.name);
    if (entry.isDirectory()) {
      walkFiles(fullPath, cb);
    } else if (entry.isFile()) {
      cb(fullPath);
    }
  }
}

function checkNoProductDemoCopy() {
  const hits = [];
  walkFiles(join(root, 'src'), filePath => {
    const rel = relative(root, filePath).replaceAll('\\', '/');
    if (isAllowedDemoModeFile(rel)) return;
    if (!/\.(ts|tsx|css|json)$/.test(filePath)) return;
    const lines = readFileSync(filePath, 'utf-8').split('\n');
    lines.forEach((line, index) => {
      if (/\bdemo\b/i.test(line)) {
        hits.push(`${relative(root, filePath)}:${index + 1}: ${line.trim()}`);
      }
    });
  });
  if (hits.length > 0) {
    throw new Error(`Product demo copy is not allowed in src:\n${hits.join('\n')}`);
  }
}

function isAllowedDemoModeFile(rel) {
  return (
    rel === 'src/lib/demoMode.ts' ||
    rel === 'src/lib/demoMode.test.ts' ||
    rel === 'src/pages/settings/ProvidersSection.tsx' ||
    rel === 'src/components/FloatingShell.tsx' ||
    /^src\/i18n\/[^/]+\.ts$/.test(rel)
  );
}

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

function runCheck(label, fn) {
  process.stdout.write(`  ${label} ... `);
  try {
    fn();
    console.log('OK');
    return true;
  } catch (e) {
    console.log('FAIL');
    const message = e instanceof Error ? e.message : String(e);
    const lines = message.split('\n').filter(l => l.trim());
    const show = lines.slice(-10).join('\n');
    if (show) console.error(show.split('\n').map(l => '    ' + l).join('\n'));
    failed++;
    return false;
  }
}

console.log('=== Frontend Verification ===\n');

// Phase 1: Static analysis (fast)
runCheck('no product demo copy', checkNoProductDemoCopy);
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
