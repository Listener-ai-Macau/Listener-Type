#!/usr/bin/env node
import fs from 'node:fs';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

const root = process.cwd();
const required = [
  'LICENSE', 'README.md', 'README.zh.md', 'CONTRIBUTING.md',
  'CODE_OF_CONDUCT.md', 'SECURITY.md', 'SUPPORT.md', 'docs/OPEN_SOURCE.md',
  '.github/pull_request_template.md', '.github/ISSUE_TEMPLATE/bug.yml',
  '.github/ISSUE_TEMPLATE/feature.yml', '.github/ISSUE_TEMPLATE/question.yml',
  '.github/ISSUE_TEMPLATE/config.yml', '.github/ISSUE_TEMPLATE/security-contact.yml',
];

function fail(message) {
  console.error(`FAIL: ${message}`);
  process.exitCode = 1;
}

for (const file of required) {
  if (!fs.existsSync(path.join(root, file))) fail(`missing required open-source file: ${file}`);
}

const linkedDocuments = ['README.md', 'README.zh.md', 'CONTRIBUTING.md', 'SECURITY.md', 'SUPPORT.md', 'docs/OPEN_SOURCE.md'];
const linkPattern = /\[[^\]]+\]\(([^)]+)\)/g;
for (const file of linkedDocuments) {
  const source = fs.readFileSync(path.join(root, file), 'utf8');
  for (const match of source.matchAll(linkPattern)) {
    const target = match[1].split('#', 1)[0].trim();
    if (!target || /^(?:https?:|mailto:)/i.test(target)) continue;
    if (!fs.existsSync(path.resolve(root, path.dirname(file), target))) fail(`${file} links to missing path: ${target}`);
  }
}

const gitignore = fs.readFileSync(path.join(root, '.gitignore'), 'utf8');
for (const entry of ['node_modules/', 'dist/', '**/target/', '.env', '.env.*', '.cache/', '.artifacts/']) {
  if (!gitignore.includes(entry)) fail(`.gitignore is missing ${entry}`);
}

const publicSurface = [
  'README.md', 'README.zh.md', 'CONTRIBUTING.md', 'CODE_OF_CONDUCT.md',
  'SECURITY.md', 'SUPPORT.md', 'LICENSE', 'docs/OPEN_SOURCE.md', '.gitmodules',
  'package.json',
  ...fs.readdirSync(path.join(root, '.github'), { recursive: true }).map((item) => `.github/${item}`),
];
const secretPattern = /-----BEGIN [A-Z ]+ PRIVATE KEY-----|\b(?:sk-[A-Za-z0-9]{20,}|ghp_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,})\b/;
const localPathPattern = /C:\\Users\\(?:Billy|OWNER)|\/Users\/(?:[A-Za-z0-9._-]+)\//;
for (const file of publicSurface) {
  const absolute = path.join(root, file);
  if (!fs.existsSync(absolute) || fs.statSync(absolute).isDirectory()) continue;
  const source = fs.readFileSync(absolute, 'utf8');
  if (secretPattern.test(source)) fail(`possible credential material in public surface: ${file}`);
  if (localPathPattern.test(source)) fail(`developer-local path in public surface: ${file}`);
}

const gitmodules = fs.readFileSync(path.join(root, '.gitmodules'), 'utf8');
if (/url\s*=\s*git@github\.com:/i.test(gitmodules)) fail('.gitmodules uses an SSH-only URL; use HTTPS for public cloning');

const openSourceStatus = fs.readFileSync(path.join(root, 'docs/OPEN_SOURCE.md'), 'utf8');
if (!/Current release status:\s*BLOCKED/i.test(openSourceStatus)) fail('docs/OPEN_SOURCE.md must state the current external dependency release status');
for (const file of ['README.md', 'README.zh.md', 'CONTRIBUTING.md']) {
  if (!/BLOCKED/i.test(fs.readFileSync(path.join(root, file), 'utf8'))) fail(`${file} must repeat the current blocked release boundary`);
}

const tracked = spawnSync('git', ['ls-files', '-z'], { cwd: root, encoding: 'utf8' }).stdout.split('\0').filter(Boolean);
for (const file of tracked) {
  const isGeneratedEvidence = /(^|\/)docs\/validation\/|(^|\/)docs\/release\/evidence\/.*\.(?:log|jsonl)$/i.test(file);
  if (isGeneratedEvidence && fs.existsSync(path.join(root, file))) fail(`generated evidence is still tracked: ${file}`);
}

const workflowsRoot = path.join(root, '.github/workflows');
for (const file of fs.readdirSync(workflowsRoot).filter((item) => /\.(yml|yaml)$/.test(item))) {
  const source = fs.readFileSync(path.join(workflowsRoot, file), 'utf8');
  if (!/^name:\s*\S+/m.test(source) || !/^jobs:\s*$/m.test(source)) fail(`workflow is missing name/jobs: .github/workflows/${file}`);
}
const ciWorkflow = fs.readFileSync(path.join(workflowsRoot, 'ci.yml'), 'utf8');
const publicJob = ciWorkflow.split(/\n  (?:frontend-and-audits|rust-check):/m, 1)[0];
if (!/^  public-hygiene:/m.test(publicJob)) fail('ci.yml must expose a public-hygiene job');
if (/DENZIC_PLATFORM_DEPLOY_KEY/i.test(publicJob)) fail('public-hygiene job must not require the private platform deploy key');

if (process.exitCode) process.exit();
console.log('PASS: open-source repository hygiene checks passed.');
