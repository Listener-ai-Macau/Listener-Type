#!/usr/bin/env node
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import process from 'node:process';
import { fileURLToPath } from 'node:url';

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.dirname(scriptDir);
const catalogPath = path.join(scriptDir, 'multi-speaker-timeline-scenarios.json');
const outputPath = process.argv[2]
  ? path.resolve(process.argv[2])
  : path.join(repoRoot, '.artifacts', 'multi-speaker-timeline', 'report.json');
const catalogBytes = readFileSync(catalogPath);
const catalog = JSON.parse(catalogBytes);
const volcengineSource = readFileSync(
  path.join(repoRoot, 'src-tauri', 'src', 'asr', 'volcengine.rs'),
  'utf8',
);
const dictationTestsSource = readFileSync(
  path.join(repoRoot, 'src-tauri', 'src', 'coordinator', 'dictation_tests.rs'),
  'utf8',
);

function fail(message) {
  console.error(`FAIL: ${message}`);
  process.exit(1);
}

function hasRustTest(source, name) {
  const escaped = name.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  return new RegExp(`\\#\\[test\\][\\s\\S]{0,160}fn\\s+${escaped}\\s*\\(`).test(source);
}

if (catalog.schema !== 'listener.multi_speaker_timeline_scenarios' || catalog.schema_version !== 1) {
  fail('invalid multi-speaker timeline catalog schema');
}
if (!Array.isArray(catalog.scenarios) || catalog.scenarios.length < 10) {
  fail('at least 10 explicit multi-speaker scenarios are required');
}
const ids = catalog.scenarios.map((scenario) => scenario.id);
if (new Set(ids).size !== ids.length) {
  fail('multi-speaker scenario IDs must be unique');
}
if (catalog.scenarios.filter((scenario) => scenario.kind === 'overlap').length < 3) {
  fail('at least three overlap scenarios are required');
}
for (const scenario of catalog.scenarios) {
  if (!['alternating', 'overlap'].includes(scenario.kind)) {
    fail(`${scenario.id}: kind must be alternating or overlap`);
  }
  if (!hasRustTest(volcengineSource, scenario.transcript_test)) {
    fail(`${scenario.id}: transcript regression test is missing: ${scenario.transcript_test}`);
  }
  if (!hasRustTest(dictationTestsSource, scenario.endpoint_test)) {
    fail(`${scenario.id}: endpoint regression test is missing: ${scenario.endpoint_test}`);
  }
}
if (!hasRustTest(dictationTestsSource, catalog.lifecycle_test)) {
  fail(`lifecycle regression test is missing: ${catalog.lifecycle_test}`);
}

function runCargo(filter) {
  const result = spawnSync(
    'cargo',
    [
      'test',
      '--manifest-path',
      'src-tauri/Cargo.toml',
      '--lib',
      filter,
      '--',
      '--test-threads=1',
    ],
    {
      cwd: repoRoot,
      encoding: 'utf8',
      env: { ...process.env, CARGO_TERM_COLOR: 'never', LISTENER_TYPE_DISABLE_BACKGROUND_BLE: '1' },
      maxBuffer: 64 * 1024 * 1024,
    },
  );
  const output = `${result.stdout ?? ''}${result.stderr ?? ''}`;
  process.stdout.write(output);
  if (result.error || result.status !== 0) {
    fail(`${filter} Rust regressions failed${result.error ? `: ${result.error.message}` : ''}`);
  }
  return output;
}

const volcengineOutput = runCargo('asr::volcengine::tests');
const dictationOutput = runCargo('coordinator::dictation::tests');

function outputHasPassedTest(output, moduleName, testName) {
  return output
    .split(/\r?\n/)
    .some((line) => line.includes(`test ${moduleName}::${testName} ... ok`));
}

const lifecyclePassed = outputHasPassedTest(
    dictationOutput,
  'coordinator::dictation::tests',
  catalog.lifecycle_test,
);
const scenarios = catalog.scenarios.map((scenario) => ({
  id: scenario.id,
  kind: scenario.kind,
  transcript_test: scenario.transcript_test,
  endpoint_test: scenario.endpoint_test,
  transcript_pass: outputHasPassedTest(
    volcengineOutput,
    'asr::volcengine::tests',
    scenario.transcript_test,
  ),
  endpoint_pass: outputHasPassedTest(
    dictationOutput,
    'coordinator::dictation::tests',
    scenario.endpoint_test,
  ),
  lifecycle_pass: lifecyclePassed,
}));
for (const scenario of scenarios) {
  scenario.pass = scenario.transcript_pass && scenario.endpoint_pass && scenario.lifecycle_pass;
}
const report = {
  schema: 'listener.multi_speaker_timeline_report',
  schema_version: 1,
  catalog_sha256: createHash('sha256').update(catalogBytes).digest('hex').toUpperCase(),
  scenario_count: scenarios.length,
  alternating_count: scenarios.filter((scenario) => scenario.kind === 'alternating').length,
  overlap_count: scenarios.filter((scenario) => scenario.kind === 'overlap').length,
  raw_audio_retained: false,
  transcript_body_retained: false,
  pass: scenarios.every((scenario) => scenario.pass),
  scenarios,
};
mkdirSync(path.dirname(outputPath), { recursive: true });
writeFileSync(outputPath, `${JSON.stringify(report, null, 2)}\n`);
if (!report.pass) {
  fail(`one or more multi-speaker scenarios failed; report=${outputPath}`);
}
console.log(`PASS: ${report.scenario_count} multi-speaker timeline regressions; report=${outputPath}`);
