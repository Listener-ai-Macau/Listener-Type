#!/usr/bin/env node
import { spawnSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const scriptsDir = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(scriptsDir, '..');

function runGit(args) {
  const result = spawnSync('git', args, {
    cwd: repoRoot,
    encoding: 'utf8',
  });
  if (result.error) {
    throw result.error;
  }
  if (result.status !== 0) {
    const detail = (result.stderr || result.stdout || '').trim();
    throw new Error(`git ${args.join(' ')} failed${detail ? `: ${detail}` : ''}`);
  }
  return result.stdout.replace(/\r\n/g, '\n').trimEnd();
}

function shouldScan(relativePath) {
  const rel = relativePath.replaceAll('\\', '/');
  if (rel.startsWith('docs/validation/') || rel.startsWith('docs/plans/archive/')) {
    return false;
  }
  if (['README.md', 'README.zh.md', 'CONTRIBUTING.md', 'CLAUDE.md'].includes(rel)) {
    return true;
  }
  if (rel.startsWith('docs/') && /\.(md|json)$/i.test(rel)) {
    return true;
  }
  if (rel.startsWith('.github/workflows/') && /\.(ya?ml)$/i.test(rel)) {
    return true;
  }
  if (rel.startsWith('tools/') && /\.(ps1|py|md|json)$/i.test(rel)) {
    return true;
  }
  return rel === 'scripts/windows-package-msvc.cmd'
    || rel === 'src-tauri/src/asr/local/foundry_native.rs';
}

const forbidden = [
  {
    pattern: /(^|\s)powershell(?:\.exe)?\s+-(NoProfile|ExecutionPolicy|File|Command)\b/im,
    message: 'use pwsh -NoProfile -File instead of Windows PowerShell entrypoints',
  },
  {
    pattern: /&\s*powershell(?:\.exe)?\b/i,
    message: 'invoke pwsh instead of powershell.exe',
  },
  {
    pattern: /\bStart-Process\s+-FilePath\s+["']powershell(?:\.exe)?["']/i,
    message: 'Start-Process must use pwsh.exe for script helpers',
  },
  {
    pattern: /["']-ExecutionPolicy["']|\s-ExecutionPolicy\s/i,
    message: 'do not carry execution-policy shims in pwsh entrypoints',
  },
  {
    pattern: /shutil\.which\(["']powershell["']\)/i,
    message: 'do not fall back to Windows PowerShell when pwsh is missing',
  },
];

const tracked = runGit(['ls-files'])
  .split('\n')
  .map((line) => line.trim())
  .filter(Boolean);

const failures = [];
for (const rel of tracked.filter(shouldScan)) {
  const text = readFileSync(join(repoRoot, rel), 'utf8');
  for (const rule of forbidden) {
    const match = rule.pattern.exec(text);
    if (match) {
      failures.push(`${rel}: ${rule.message}: ${JSON.stringify(match[0])}`);
      break;
    }
  }
}

if (failures.length > 0) {
  console.error('FAIL: Type public/tool entrypoints still contain legacy PowerShell invocation patterns.');
  for (const failure of failures) {
    console.error(`  - ${failure}`);
  }
  process.exit(1);
}

console.log('PASS: Type public/tool entrypoints use pwsh and do not fall back to Windows PowerShell.');
