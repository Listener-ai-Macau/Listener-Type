import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
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
const appRoot = join(scriptsDir, '..');
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

function runManifest(extraEnv = {}) {
  const result = spawnSync(
    process.execPath,
    [join(scriptsDir, 'write-updater-manifest.mjs')],
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

cleanup();
