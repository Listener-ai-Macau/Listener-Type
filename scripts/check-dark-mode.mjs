#!/usr/bin/env node
// check-dark-mode.mjs — Verify dark mode CSS variable coverage and TSX color hygiene.
//
// Checks:
//   1. Every custom property in :root also exists in [data-theme="dark"]
//   2. No hardcoded white/light backgrounds in TSX components (excluding Capsule.tsx)
//   3. useDarkMode hook IIFE is actually invoked (has closing "()()")
//
// Usage: node scripts/check-dark-mode.mjs

import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';
import process from 'node:process';

const root = process.cwd();
let errors = 0;

// ── 1. CSS variable completeness ──────────────────────────────────────────────

const tokensPath = join(root, 'src/styles/tokens.css');
const tokensCss = readFileSync(tokensPath, 'utf-8');

function extractVars(selector) {
  const re = new RegExp(String.raw`${selector}\s*\{([^}]+)\}`, 's');
  const m = tokensCss.match(re);
  if (!m) return new Set();
  const vars = new Set();
  for (const [, name] of m[1].matchAll(/(--[\w-]+)/g)) {
    vars.add(name);
  }
  return vars;
}

const rootVars = extractVars(':root');
const darkVars = extractVars('\\[data-theme="dark"\\]');

const missingInDark = [...rootVars].filter(v => !darkVars.has(v));
// Non-visual vars that don't need dark overrides
const allowedMissing = new Set([
  '--ol-glass-blur', '--ol-glass-blur-strong',           // blur amounts
  '--ol-motion-spring', '--ol-motion-soft', '--ol-motion-quick', // motion curves
  '--ol-r-sm', '--ol-r-md', '--ol-r-lg', '--ol-r-xl', '--ol-r-2xl', '--ol-r-pill', // radii
  '--ol-font-sans', '--ol-font-mono',                     // fonts
  '--ol-window-shell-radius', '--ol-window-console-radius', '--ol-window-titlebar-height', // window
]);

for (const v of missingInDark) {
  if (allowedMissing.has(v)) continue;
  console.error(`MISSING DARK VAR: ${v} defined in :root but not in [data-theme="dark"]`);
  errors++;
}

// ── 2. Hardcoded white backgrounds in TSX ─────────────────────────────────────

const excludedFiles = new Set([
  'Capsule.tsx',  // intentionally stays light
]);

// Patterns that indicate a hardcoded light/white background
const badBackgroundPatterns = [
  /background:\s*'rgba\(255,\s*255,\s*255/,   // rgba white
  /background:\s*'#fff\b/,                     // #fff
  /background:\s*'#ffffff'/i,                  // #ffffff
  /background:\s*'#fffffb'/i,                  // #fffffb (token white raw)
  /background:\s*'white'/,                     // 'white'
];

function walkDir(dir, cb) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    if (entry.name.startsWith('.') || entry.name === 'node_modules' || entry.name === 'target') continue;
    const fullPath = join(dir, entry.name);
    if (entry.isDirectory()) {
      walkDir(fullPath, cb);
    } else if (entry.isFile() && entry.name.endsWith('.tsx')) {
      cb(fullPath);
    }
  }
}

walkDir(join(root, 'src'), (filePath) => {
  const rel = relative(root, filePath);
  if (excludedFiles.has(rel.split(/[/\\]/).pop())) return;

  const content = readFileSync(filePath, 'utf-8');
  const lines = content.split('\n');

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    for (const pat of badBackgroundPatterns) {
      if (pat.test(line)) {
        // Skip if it's a text color (color: '#fff' on dark bg is fine)
        if (/color:\s*'/.test(line) && !/background/.test(line)) continue;
        console.error(`HARDCODED COLOR: ${rel}:${i + 1} — ${line.trim()}`);
        errors++;
      }
    }
  }
});

// ── 3. useDarkMode IIFE invocation ────────────────────────────────────────────

const appPath = join(root, 'src/App.tsx');
const appContent = readFileSync(appPath, 'utf-8');

if (!appContent.includes('})()') || !appContent.includes('dark-mode-changed')) {
  console.error('IIFE BUG: useDarkMode async IIFE not invoked — check for missing ()() in App.tsx');
  errors++;
}

// ── Result ────────────────────────────────────────────────────────────────────

if (errors > 0) {
  console.error(`\nFAIL: ${errors} issue(s) found`);
  process.exit(1);
}

console.log('PASS: dark mode CSS complete, no hardcoded white backgrounds, IIFE invoked');
process.exit(0);
