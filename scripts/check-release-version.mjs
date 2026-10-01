#!/usr/bin/env node
import { spawnSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const scriptDir = dirname(fileURLToPath(import.meta.url));
const configIndex = process.argv.indexOf('--config');
const configPath = configIndex >= 0 ? resolve(process.argv[configIndex + 1]) : join(scriptDir, 'release-version-gate.json');

function readVersion(root, source) {
  const content = readFileSync(resolve(root, source.path), 'utf8');
  if (source.kind === 'text') return content.trim();
  if (source.kind === 'regex') return content.match(new RegExp(source.pattern, 'm'))?.[1];
  if (source.kind === 'json') return source.key.split('.').reduce((value, key) => value?.[key], JSON.parse(content));
  throw new Error(`Unsupported version source kind: ${source.kind}`);
}

try {
  const config = JSON.parse(readFileSync(configPath, 'utf8'));
  if (config.schema !== 'denzic.platform.release-version-gate.v1') throw new Error('Unsupported release version configuration');
  const root = resolve(dirname(configPath), config.root || '.');
  const version = readVersion(root, config.canonical);
  const base = '(?:0|[1-9][0-9]*)';
  const stable = `${base}\\.${base}\\.${base}`;
  const candidate = version?.match(new RegExp(`^${stable}-([a-z]+)\\.(${base})$`));
  if (!new RegExp(`^${stable}$`).test(version) && !config.prerelease_channels?.includes(candidate?.[1])) {
    throw new Error(config.semver_error.replace('{version}', version));
  }
  for (const source of config.must_equal || []) {
    const actual = readVersion(root, source);
    if (actual !== version) throw new Error(`${source.label}: ${actual}; expected ${version}`);
  }
  for (const source of config.must_contain || []) {
    const content = readFileSync(resolve(root, source.path), 'utf8');
    if (!content.includes(version) || (source.reference && !content.includes(source.reference))) {
      throw new Error(`${source.path} does not contain the required version/reference`);
    }
  }
  if (process.argv.includes('--require-tag')) {
    const tag = `${config.tag_prefix || 'v'}${version}`;
    const args = config.tag_mode === 'exists' ? ['tag', '--list', tag] : ['describe', '--tags', '--exact-match'];
    const result = spawnSync('git', args, { cwd: root, encoding: 'utf8' });
    if (result.error) throw result.error;
    if (result.status !== 0 || result.stdout.trim() !== tag) throw new Error(`Current version requires git tag ${tag}`);
  }
  console.log(`PASS: ${config.product} release version is ${version}`);
} catch (error) {
  console.error(error.message);
  process.exit(1);
}
