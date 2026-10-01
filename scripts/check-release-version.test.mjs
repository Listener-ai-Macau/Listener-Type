import assert from 'node:assert/strict';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const fixture = mkdtempSync(join(tmpdir(), 'listener-version-gate-'));
const tool = fileURLToPath(new URL('./check-release-version.mjs', import.meta.url));
try {
  const cases = [
    ['1.0.6', [], '1.0.6', true],
    ['1.0.6-beta.4', [], '1.0.6-beta.4', false],
    ['1.0.6-beta.4', ['beta', 'rc'], '1.0.6-beta.4', true],
    ['1.0.6-rc.1', ['beta', 'rc'], '1.0.6-rc.1', true],
    ['1.0.6-beta.04', ['beta', 'rc'], '1.0.6-beta.04', false],
    ['1.0.6-preview.4', ['beta', 'rc'], '1.0.6-preview.4', false],
    ['1.0.6-beta.4', ['beta', 'rc'], '1.0.6', false],
  ];
  for (const [version, channels, companion, passes] of cases) {
    writeFileSync(join(fixture, 'package.json'), JSON.stringify({ version }));
    writeFileSync(join(fixture, 'companion.json'), JSON.stringify({ version: companion }));
    writeFileSync(join(fixture, 'gate.json'), JSON.stringify({
      schema: 'denzic.platform.release-version-gate.v1', product: 'Listener Type', root: '.',
      canonical: { path: 'package.json', kind: 'json', key: 'version' },
      semver_error: 'Invalid version: {version}', prerelease_channels: channels,
      must_equal: [{ label: 'companion', path: 'companion.json', kind: 'json', key: 'version' }],
    }));
    const result = spawnSync(process.execPath, [tool, '--config', join(fixture, 'gate.json')], { encoding: 'utf8' });
    assert.ifError(result.error);
    assert.equal(result.status === 0, passes, `${version}: ${result.stdout}${result.stderr}`);
  }
  console.log('PASS: stable/candidate version consistency and stable-only default');
} finally {
  rmSync(fixture, { recursive: true, force: true });
}
