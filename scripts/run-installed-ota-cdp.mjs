#!/usr/bin/env node
import { writeFileSync } from "node:fs";

function takeArg(name) {
  const index = process.argv.indexOf(name);
  const value = index >= 0 ? process.argv[index + 1] : null;
  if (!value || value.startsWith("--")) {
    throw new Error(`${name} requires a value`);
  }
  return value;
}

const cdpUrl = takeArg("--cdp-url");
const packagePath = takeArg("--package");
const outputPath = takeArg("--output-json");
const coldStart = process.argv.includes("--cold-start");

function writeResult(value) {
  writeFileSync(outputPath, `${JSON.stringify(value, null, 2)}\n`, "utf8");
}

function normalizedManifest(raw) {
  const firmware = raw.firmware;
  const requirements = raw.requirements;
  const protocol = raw.protocol;
  const gatt = protocol.gatt;
  const rollback = raw.rollback;
  const recovery = raw.recovery;
  return {
    schemaVersion: raw.schema_version ?? raw.schemaVersion,
    // The release manifest omits this internal discriminator; the production
    // frontend supplies the fixed Listener package type before invoking Rust.
    packageType: "listener-firmware-ota",
    project: firmware.project,
    version: firmware.version,
    protocolName: protocol.name,
    protocolVersion: protocol.version,
    hardwareRevision: requirements.hardware_revision ?? requirements.hardwareRevision,
    minDesktopVersion: requirements.min_desktop_version ?? requirements.minDesktopVersion,
    channel: raw.channel,
    fileName: firmware.file,
    fileSizeBytes: firmware.size_bytes ?? firmware.sizeBytes,
    fileSha256: (firmware.sha256 ?? "").toLowerCase(),
    firmwareCapability: protocol.firmware_capability ?? protocol.firmwareCapability,
    gattServiceUuid: gatt.service_uuid ?? gatt.serviceUuid,
    gattControlUuid: gatt.control_uuid ?? gatt.controlUuid,
    gattDataUuid: gatt.data_uuid ?? gatt.dataUuid,
    gattConfirmUuid: gatt.confirm_uuid ?? gatt.confirmUuid ?? null,
    gattStatusUuid: gatt.status_uuid ?? gatt.statusUuid ?? null,
    gattChunkBytes: gatt.chunk_bytes ?? gatt.chunkBytes ?? 500,
    rollbackInstructions:
      typeof rollback.instructions === "string"
        ? [rollback.instructions.trim()]
        : rollback.instructions,
    recoveryInstructions: [
      recovery.factory_reflash ?? recovery.factoryReflash,
      recovery.serial_commands ?? recovery.serialCommands,
    ],
  };
}

let socket;
try {
  socket = new WebSocket(cdpUrl);
  await new Promise((resolve, reject) => {
    const timeout = setTimeout(
      () => reject(new Error("CDP WebSocket open timed out")),
      5000,
    );
    socket.addEventListener(
      "open",
      () => {
        clearTimeout(timeout);
        resolve();
      },
      { once: true },
    );
    socket.addEventListener(
      "error",
      event => {
        clearTimeout(timeout);
        reject(
          new Error(
            `CDP WebSocket open failed: ${String(event.message ?? event.type)}`,
          ),
        );
      },
      { once: true },
    );
  });

  let nextId = 1;
  const pending = new Map();
  socket.addEventListener("message", event => {
    const message = JSON.parse(String(event.data));
    if (!message.id || !pending.has(message.id)) return;
    const { resolve, reject } = pending.get(message.id);
    pending.delete(message.id);
    if (message.error) reject(new Error(JSON.stringify(message.error)));
    else resolve(message.result);
  });

  const cdp = (method, params = {}) =>
    new Promise((resolve, reject) => {
      const id = nextId++;
      pending.set(id, { resolve, reject });
      socket.send(JSON.stringify({ id, method, params }));
    });
  const evaluate = async expression => {
    const response = await cdp("Runtime.evaluate", {
      expression,
      awaitPromise: true,
      returnByValue: true,
      userGesture: false,
    });
    if (response.exceptionDetails) {
      throw new Error(
        response.exceptionDetails.text ?? JSON.stringify(response.exceptionDetails),
      );
    }
    return response.result.value;
  };

  const remoteExpression = `
    (async () => {
      const invoke = window.__TAURI__?.core?.invoke;
      if (!invoke) throw new Error("Tauri invoke bridge unavailable");
      const pause = milliseconds => new Promise(resolve => setTimeout(resolve, milliseconds));
      let runtimeBeforeStart = null;
      let typeReadyBeforeStart = false;
      for (let attempt = 0; !${coldStart} && attempt < 150; attempt += 1) {
        runtimeBeforeStart = await invoke("get_embedded_ble_runtime_status");
        if (runtimeBeforeStart.backgroundListenerReady) {
          typeReadyBeforeStart = true;
          break;
        }
        await pause(100);
      }
      if (!${coldStart} && !typeReadyBeforeStart) {
        throw new Error("background Listener Type notify subscription was not ready before OTA");
      }
      if (${coldStart}) {
        runtimeBeforeStart = await invoke("get_embedded_ble_runtime_status");
      }
      const payload = await invoke("load_firmware_ota_package", { path: ${JSON.stringify(packagePath)} });
      const raw = JSON.parse(payload.manifestText);
      const manifest = ${normalizedManifest.toString()}(raw);
      const progress = [];
      const unlisten = await window.__TAURI__.event.listen("firmware-ota:progress", event => {
        progress.push({ atMs: Date.now(), ...event.payload });
      });
      let result = null;
      let commandError = null;
      try {
        result = await invoke("transfer_firmware_ota_ble", {
          manifest,
          firmwareBytes: payload.firmwareBytes,
          expectedSha256: manifest.fileSha256,
        });
      } catch (error) {
        commandError = error instanceof Error ? error.message : String(error);
      } finally {
        unlisten();
      }
      return {
        typeReadyBeforeStart,
        coldStart: ${coldStart},
        runtimeBeforeStart,
        manifest: {
          schemaVersion: manifest.schemaVersion,
          packageType: manifest.packageType,
          protocolName: manifest.protocolName,
          version: manifest.version,
          fileSizeBytes: manifest.fileSizeBytes,
          fileSha256: manifest.fileSha256,
        },
        result,
        commandError,
        progress,
      };
    })()
  `;
  const outcome = await evaluate(remoteExpression);
  writeResult({ status: outcome.commandError ? "FAIL" : "PASS", ...outcome });
} catch (error) {
  writeResult({
    status: "FAIL",
    cdpError: error instanceof Error ? error.message : String(error),
  });
  process.exitCode = 1;
} finally {
  socket?.close();
}
