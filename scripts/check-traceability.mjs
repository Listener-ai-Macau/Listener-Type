#!/usr/bin/env node
import { existsSync, mkdirSync, readFileSync, readdirSync, statSync, writeFileSync } from 'node:fs';
import { dirname, join, relative } from 'node:path';
import process from 'node:process';

const root = process.cwd();
const traceFile = 'specs/traceability/files.md';
const writeMode = process.argv.includes('--write');

const trackedRoots = [
  'src',
  'src-tauri/src',
  'src-tauri/backend-tests',
  'src-tauri/nsis',
  'src-tauri/vendor/qwen-asr',
  'src-tauri/wix',
  'scripts',
  'windows-ime',
];
const trackedFiles = [
  'package.json',
  'package-lock.json',
  'index.html',
  'vite.config.ts',
  'tsconfig.json',
  'tsconfig.node.json',
  'src-tauri/Cargo.toml',
  'src-tauri/tauri.conf.json',
  'src-tauri/Info.plist',
  'src-tauri/Entitlements.plist',
];
const ignoredDirs = new Set(['node_modules', 'target', '.git']);
const codeExts = new Set([
  '.c', '.cmd', '.cpp', '.css', '.def', '.h', '.html', '.js', '.json', '.mjs',
  '.plist', '.ps1', '.py', '.rc', '.rs', '.sln', '.toml', '.ts', '.tsx',
  '.vcxproj', '.wxs', '.sh',
]);

function extname(path) {
  const index = path.lastIndexOf('.');
  return index === -1 ? '' : path.slice(index);
}

function walk(dir, out = []) {
  if (!existsSync(dir)) return out;
  for (const entry of readdirSync(dir)) {
    if (ignoredDirs.has(entry)) continue;
    const full = join(dir, entry);
    const rel = relative(root, full).replaceAll('\\', '/');
    const st = statSync(full);
    if (st.isDirectory()) {
      walk(full, out);
    } else if (codeExts.has(extname(rel)) || rel.endsWith('/Makefile')) {
      out.push(rel);
    }
  }
  return out;
}

function areaFor(file) {
  if (file.startsWith('src-tauri/src/asr')) return ['ASR providers', 'Provider tests, dictation smoke'];
  if (file.startsWith('src-tauri/vendor/qwen-asr')) return ['Vendored local ASR engine', 'Cargo check and local ASR smoke'];
  if (file.startsWith('src-tauri/src/llm') || file.includes('/polish')) return ['Polish providers', 'Rust unit tests, provider smoke'];
  if (file.includes('windows_ime') || file.startsWith('windows-ime') || file.includes('listener-type-ime')) return ['Windows IME insertion', 'Windows static and runtime smoke'];
  if (file.includes('hotkey')) return ['Global hotkeys', 'Hotkey tests and manual smoke'];
  if (file.includes('persistence') || file.includes('style_pack')) return ['Local data and style packs', 'Rust unit tests, local import/export smoke'];
  if (file.startsWith('src/pages') || file.startsWith('src/components')) return ['React UI', 'TypeScript build and visual smoke'];
  if (file.startsWith('src/i18n')) return ['Localization', 'TypeScript build'];
  if (file.startsWith('src/styles')) return ['Design system', 'Visual smoke'];
  if (file.startsWith('scripts')) return ['Automation and audits', 'Node/PowerShell script tests'];
  if (file.includes('tauri.conf') || file.includes('Cargo.toml') || file.includes('package')) return ['Build and release config', 'Build, updater and audit scripts'];
  return ['Core app', 'Build and targeted tests'];
}

function collectFiles() {
  const files = new Set(trackedFiles.filter(existsSync));
  for (const dir of trackedRoots) {
    for (const file of walk(join(root, dir))) files.add(file);
  }
  return [...files].sort();
}

function generate(files) {
  const rows = files.map(file => {
    const [area, verification] = areaFor(file);
    return `| \`${file}\` | ${area} | ${verification} |`;
  });
  return `# File Traceability\n\nEvery tracked code/config/script file must be listed here. Regenerate with \`npm run check:traceability -- --write\`.\n\n| File | Responsibility | Verification |\n| --- | --- | --- |\n${rows.join('\n')}\n`;
}

const files = collectFiles();

if (writeMode) {
  mkdirSync(dirname(traceFile), { recursive: true });
  writeFileSync(traceFile, generate(files));
  console.log(`Wrote ${traceFile} with ${files.length} tracked files.`);
  process.exit(0);
}

if (!existsSync(traceFile)) {
  console.error(`${traceFile} is missing. Run npm run check:traceability -- --write.`);
  process.exit(1);
}

const text = readFileSync(traceFile, 'utf8');
const missing = files.filter(file => !text.includes(`\`${file}\``));
if (missing.length) {
  console.error('Traceability check failed. Missing files:');
  console.error(missing.join('\n'));
  process.exit(1);
}

console.log(`Traceability check passed for ${files.length} tracked files.`);
