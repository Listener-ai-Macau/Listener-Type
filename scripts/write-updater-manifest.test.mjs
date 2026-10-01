import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { tmpdir } from 'node:os';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

function assert(condition, message) {
  if (!condition) {
    throw new Error(message);
  }
}

function readJson(path) {
  return JSON.parse(readFileSync(path, 'utf8'));
}

const scriptsDir = dirname(fileURLToPath(import.meta.url));
const appRoot = mkdtempSync(join(tmpdir(), 'listener-updater-gate-'));
const fixtureScripts = join(appRoot, 'scripts');
mkdirSync(fixtureScripts);
copyFileSync(join(scriptsDir, 'write-updater-manifest.mjs'), join(fixtureScripts, 'write-updater-manifest.mjs'));
writeFileSync(join(appRoot, 'package.json'), JSON.stringify({ version: '1.0.6' }));
const bundleDir = join(appRoot, 'src-tauri', 'target', 'release', 'bundle');
const macosDir = join(bundleDir, 'macos');
const artifact = join(macosDir, 'ListenerType_aarch64.app.tar.gz');
const signature = `${artifact}.sig`;
const stableManifest = join(bundleDir, 'latest-darwin-aarch64.json');
const mirrorManifest = join(bundleDir, 'latest-darwin-aarch64-mirror.json');
const upstreamRepoNeedle = ['appergb', ['open', 'less'].join('')].join('/');

function cleanup() {
  for (const path of [artifact, signature, stableManifest, mirrorManifest]) {
    rmSync(path, { force: true });
  }
}

function runManifest(extraEnv = {}, expectSuccess = true) {
  const result = spawnSync(
    process.execPath,
    [join(fixtureScripts, 'write-updater-manifest.mjs')],
    {
      cwd: appRoot,
      encoding: 'utf8',
      env: {
        ...process.env,
        LISTENER_TYPE_UPDATE_TARGET: 'darwin',
        LISTENER_TYPE_UPDATE_ARCH: 'aarch64',
        LISTENER_TYPE_UPDATE_REPO: 'Listener-ai-Macau/Listener-Type',
        LISTENER_TYPE_UPDATE_MIRROR_BASE_URL: '',
        ...extraEnv,
      },
    },
  );
  if (!expectSuccess) return result;
  if (result.status !== 0) {
    throw new Error(`manifest generation failed:\n${result.stdout}\n${result.stderr}`);
  }
  return result.stdout;
}

cleanup();
mkdirSync(macosDir, { recursive: true });
writeFileSync(artifact, 'fake listener type bundle\n');
writeFileSync(signature, 'fake-signature\n');

runManifest();
const stable = readJson(stableManifest);
assert(stable.version, 'stable manifest should include a version');
assert(
  stable.url === 'https://github.com/Listener-ai-Macau/Listener-Type/releases/latest/download/ListenerType_aarch64.app.tar.gz',
  `stable manifest should use Listener Type latest release URL, got ${stable.url}`,
);
assert(stable.signature === 'fake-signature', 'stable manifest should include the trimmed signature');
assert(!existsSync(mirrorManifest), 'mirror manifest should not be generated unless a mirror base URL is configured');

runManifest({
  LISTENER_TYPE_UPDATE_MIRROR_BASE_URL: 'https://updates.listener-type.example/',
});
const mirror = readJson(mirrorManifest);
assert(
  mirror.url === 'https://updates.listener-type.example/Listener-ai-Macau/Listener-Type/releases/latest/download/ListenerType_aarch64.app.tar.gz',
  `mirror manifest should use the explicit Listener Type mirror URL, got ${mirror.url}`,
);
assert(!stable.url.includes(upstreamRepoNeedle), 'stable manifest must not point at the upstream repository');
assert(!mirror.url.includes(upstreamRepoNeedle), 'mirror manifest must not point at the upstream repository');

const existingStable = readFileSync(stableManifest, 'utf8');
const existingMirror = readFileSync(mirrorManifest, 'utf8');
for (const version of ['1.0.6-beta.4', '1.0.6-rc.1']) {
  writeFileSync(join(appRoot, 'package.json'), JSON.stringify({ version }));
  const result = runManifest({ LISTENER_TYPE_UPDATE_MIRROR_BASE_URL: 'https://updates.listener-type.example/' }, false);
  assert(result.status !== 0, 'candidate cannot generate a stable updater manifest');
  assert(result.stderr.includes('cannot generate stable updater manifests'), result.stderr);
  assert(readFileSync(stableManifest, 'utf8') === existingStable, 'candidate must preserve stable manifest');
  assert(readFileSync(mirrorManifest, 'utf8') === existingMirror, 'candidate must preserve mirror manifest');
}

cleanup();
rmSync(appRoot, { recursive: true, force: true });
console.log('PASS: stable updater generation and beta/rc rejection');
