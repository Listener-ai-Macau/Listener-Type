#!/usr/bin/env node
import { existsSync, readFileSync } from 'node:fs';
import process from 'node:process';

const requiredDocs = [
  'README.md',
  'README.zh.md',
  'AGENTS.md',
  'CLAUDE.md',
  'docs/README.md',
  'docs/USAGE.md',
  'docs/quickstart/installation.md',
  'docs/quickstart/permissions.md',
  'docs/setup/volcengine.md',
  'docs/features/dictation-pipeline.md',
  'docs/features/style-pack-marketplace.md',
  'docs/platform/windows-build.md',
  'docs/platform/windows-ime.md',
  'docs/release/updater.md',
  'docs/release/branding-and-channels.md',
  'docs/security/tauri-csp.md',
  'specs/ARCHITECTURE.md',
  'specs/DESIGN.md',
  'specs/tech_docs/windows-platform.md',
  'specs/tech_docs/local-first-cloud-degradation.md',
  'specs/guides/build_and_release_guide.md',
  'docs/archive/upstream-provenance.md',
];

const requiredSources = [
  'ref/openless/README.md',
  'ref/openless/README.zh.md',
  'ref/openless/USAGE.md',
  'ref/openless/openless-all/README.md',
  'ref/openless/docs/volcengine-setup.md',
  'ref/openless/docs/style-pack-marketplace.md',
  'ref/openless/docs/tauri-csp.md',
  'ref/openless/.github/*',
];

const failures = [];
for (const doc of requiredDocs) {
  if (!existsSync(doc)) failures.push(`missing doc: ${doc}`);
}

if (existsSync('docs/archive/upstream-provenance.md')) {
  const provenance = readFileSync('docs/archive/upstream-provenance.md', 'utf8');
  for (const source of requiredSources) {
    if (!provenance.includes(source)) {
      failures.push(`provenance does not mention ${source}`);
    }
  }
}

if (failures.length) {
  console.error('Documentation inheritance audit failed:');
  console.error(failures.join('\n'));
  process.exit(1);
}

console.log('Documentation inheritance audit passed.');
