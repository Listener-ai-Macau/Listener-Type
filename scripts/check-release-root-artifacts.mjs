#!/usr/bin/env node
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { basename, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const appRoot = resolve(fileURLToPath(new URL("..", import.meta.url)));
const denzicRoot = resolve(appRoot, "..", "..");
const packageJson = JSON.parse(readFileSync(join(appRoot, "package.json"), "utf8"));
const version = packageJson.version;

const args = process.argv.slice(2);
function readArg(name) {
  const index = args.indexOf(name);
  return index >= 0 ? args[index + 1] : "";
}

function sha256(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex").toUpperCase();
}

function requirePackage(path, label, minimumBytes) {
  assert.ok(existsSync(path), `${label} missing: ${path}`);
  const stats = statSync(path);
  assert.ok(stats.size >= minimumBytes, `${label} too small: ${stats.size} bytes`);
  return stats;
}

function assertHashMatches(rootPath, sourcePath, label) {
  if (!sourcePath) return;
  const source = resolve(sourcePath);
  assert.ok(existsSync(source), `${label} source artifact missing: ${source}`);
  assert.equal(sha256(rootPath), sha256(source), `${label} root artifact must match source artifact hash`);
}

const expectedTypeName = `ListenerType_${version}_x64_en-US.msi`;
const expectedFirmwareName = `ListenerFirmware_${version}_ota.zip`;
const expectedTypePath = join(denzicRoot, expectedTypeName);
const expectedFirmwarePath = join(denzicRoot, expectedFirmwareName);

const rootEntries = readdirSync(denzicRoot, { withFileTypes: true })
  .filter((entry) => entry.isFile())
  .map((entry) => entry.name);

const packageLike = rootEntries.filter((name) =>
  /^(ListenerType_|ListenerFirmware_).*\.(msi|zip)$/i.test(name),
);
const forbidden = packageLike.filter((name) =>
  name !== expectedTypeName && name !== expectedFirmwareName,
);
assert.deepEqual(forbidden, [], `Denzic root contains stale or forbidden release packages: ${forbidden.join(", ")}`);

const portable = rootEntries.filter((name) => /^ListenerType_.*portable.*\.zip$/i.test(name));
assert.deepEqual(portable, [], `Type portable zip must not be shipped from Denzic root: ${portable.join(", ")}`);

const typeStats = requirePackage(expectedTypePath, "Type MSI", 1_000_000);
const firmwareStats = requirePackage(expectedFirmwarePath, "Firmware OTA zip", 100_000);

assertHashMatches(expectedTypePath, readArg("--type-source"), "Type MSI");
assertHashMatches(expectedFirmwarePath, readArg("--firmware-source"), "Firmware OTA zip");

console.log(JSON.stringify({
  status: "PASS",
  root: denzicRoot,
  version,
  type_msi: {
    file: basename(expectedTypePath),
    bytes: typeStats.size,
    sha256: sha256(expectedTypePath),
  },
  firmware_ota_zip: {
    file: basename(expectedFirmwarePath),
    bytes: firmwareStats.size,
    sha256: sha256(expectedFirmwarePath),
  },
}, null, 2));
