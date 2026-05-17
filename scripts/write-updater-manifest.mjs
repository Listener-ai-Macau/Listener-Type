#!/usr/bin/env node
import { existsSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { basename, join } from 'node:path';
import process from 'node:process';
import { fileURLToPath } from 'node:url';

const target = process.env.LISTENER_TYPE_UPDATE_TARGET;
const arch = process.env.LISTENER_TYPE_UPDATE_ARCH;
const repo = process.env.LISTENER_TYPE_UPDATE_REPO || 'Listener-ai-Macau/Listener-Type';
const mirrorBaseUrl = (process.env.LISTENER_TYPE_UPDATE_MIRROR_BASE_URL || '').trim();
// 渠道决定 manifest 文件名后缀：stable → 旧文件名（向后兼容）；beta → 加 -beta 后缀，
// 让 stable 用户的 endpoint 永远拿不到 beta 包。空 / 未设置 = stable。
const rawChannel = (process.env.LISTENER_TYPE_RELEASE_CHANNEL || 'stable').toLowerCase();
if (rawChannel !== 'stable' && rawChannel !== 'beta') {
  throw new Error(`Invalid LISTENER_TYPE_RELEASE_CHANNEL: "${rawChannel}" (expected "stable" or "beta")`);
}
const channelSuffix = rawChannel === 'beta' ? '-beta' : '';
// Beta 渠道里 manifest.url 必须指向具体 tag 路径，不能用 releases/latest——后者
// 永远是最新非-prerelease（即 Stable），Beta 用户按 url 下载会拉到 Stable 包，
// 文件名碰巧重名时下载错版本，文件名带版本号时直接 404。
// Stable 渠道沿用 releases/latest，没问题。
const releaseTag = process.env.LISTENER_TYPE_RELEASE_TAG || '';
if (rawChannel === 'beta' && !releaseTag) {
  throw new Error('LISTENER_TYPE_RELEASE_TAG is required when LISTENER_TYPE_RELEASE_CHANNEL=beta');
}

if (!target || !arch) {
  throw new Error('LISTENER_TYPE_UPDATE_TARGET and LISTENER_TYPE_UPDATE_ARCH are required');
}

const packageJson = JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8'));
const bundleDir = fileURLToPath(new URL('../src-tauri/target/release/bundle/', import.meta.url));

const candidatesByTarget = {
  darwin: [
    `macos/ListenerType_${arch}.app.tar.gz`,
    'macos/Listener Type.app.tar.gz',
  ],
  windows: ['nsis/ListenerType_*_x64-setup.exe', 'nsis/Listener Type*_x64-setup.exe'],
  linux: ['appimage/ListenerType_*.AppImage', 'appimage/Listener Type*.AppImage'],
};

function findFirst(patterns) {
  for (const pattern of patterns) {
    if (!pattern.includes('*')) {
      const path = join(bundleDir, pattern);
      if (existsSync(path)) return path;
      continue;
    }
    const [dir, namePattern] = pattern.split('/');
    const dirPath = join(bundleDir, dir);
    if (!existsSync(dirPath)) continue;
    const prefix = namePattern.split('*')[0];
    const suffix = namePattern.split('*').at(-1);
    const match = readdirSync(dirPath)
      .filter(name => name.startsWith(prefix) && name.endsWith(suffix))
      .sort()[0];
    if (match) return join(dirPath, match);
  }
}

const artifact = findFirst(candidatesByTarget[target] || []);
if (!artifact) {
  throw new Error(`No updater artifact found for ${target} in ${bundleDir}`);
}

const signaturePath = `${artifact}.sig`;
if (!existsSync(signaturePath)) {
  throw new Error(`Missing updater signature: ${signaturePath}`);
}

const assetName = basename(artifact);
const manifestName = `latest-${target}-${arch}${channelSuffix}.json`;
// Stable: releases/latest/download/<asset>（GitHub 自动重定向到最新非-prerelease）
// Beta:   releases/download/<tag>/<asset>（指定具体 tag，不被 prerelease 折叠影响）
const downloadPath = rawChannel === 'beta'
  ? `releases/download/${releaseTag}/${assetName}`
  : `releases/latest/download/${assetName}`;
const githubAssetUrl = `https://github.com/${repo}/${downloadPath}`;
const mirrorAssetUrl = `${mirrorBaseUrl.replace(/\/$/, '')}/${repo}/${downloadPath}`;
const manifest = {
  version: packageJson.version,
  pub_date: new Date().toISOString(),
  url: githubAssetUrl,
  signature: readFileSync(signaturePath, 'utf8').trim(),
};

writeFileSync(join(bundleDir, manifestName), `${JSON.stringify(manifest, null, 2)}\n`);
if (mirrorBaseUrl) {
  const mirrorManifestName = `latest-${target}-${arch}${channelSuffix}-mirror.json`;
  const mirrorAssetUrl = `${mirrorBaseUrl.replace(/\/$/, '')}/${repo}/${downloadPath}`;
  const mirrorManifest = {
    ...manifest,
    url: mirrorAssetUrl,
  };
  writeFileSync(join(bundleDir, mirrorManifestName), `${JSON.stringify(mirrorManifest, null, 2)}\n`);
  console.log(`Wrote ${manifestName} and ${mirrorManifestName} for ${assetName}`);
} else {
  console.log(`Wrote ${manifestName} for ${assetName}`);
}
