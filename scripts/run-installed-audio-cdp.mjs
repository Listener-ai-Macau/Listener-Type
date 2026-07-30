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
const outputPath = takeArg("--output-json");
const timeoutMs = Number(takeArg("--timeout-ms"));

let socket;
let keepAlive;
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
        reject(new Error(`CDP WebSocket open failed: ${String(event.type)}`));
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

  keepAlive = setInterval(() => {}, 1000);
  const response = await cdp("Runtime.evaluate", {
    expression: `
      (async () => {
        const invoke = window.__TAURI__?.core?.invoke;
        if (!invoke) throw new Error("Tauri invoke bridge unavailable");
        const before = await invoke("get_embedded_ble_runtime_status");
        let result = null;
        let commandError = null;
        try {
          result = await invoke("submit_embedded_audio_ble_stream", {
            timeoutMs: ${JSON.stringify(timeoutMs)},
          });
        } catch (error) {
          commandError = error instanceof Error ? error.message : String(error);
        }
        let after = await invoke("get_embedded_ble_runtime_status");
        for (let attempt = 0; attempt < 50 && !after.backgroundListenerReady; attempt += 1) {
          await new Promise(resolve => setTimeout(resolve, 100));
          after = await invoke("get_embedded_ble_runtime_status");
        }
        return { before, result, commandError, after };
      })()
    `,
    awaitPromise: true,
    returnByValue: true,
    userGesture: false,
  });

  if (response.exceptionDetails) {
    throw new Error(
      response.exceptionDetails.text ?? JSON.stringify(response.exceptionDetails),
    );
  }

  const outcome = response.result.value;
  const silentCaptureCompleted =
    outcome.commandError === "ASR returned empty transcript";
  const pcmResultCompleted =
    !outcome.commandError &&
    outcome.result &&
    outcome.result.reconstructedPcmBytes > 0;
  const passed = Boolean(
    (pcmResultCompleted || silentCaptureCompleted) &&
      outcome.after.backgroundListenerReady,
  );
  writeFileSync(
    outputPath,
    `${JSON.stringify(
      {
        status: passed ? "PASS" : "FAIL",
        silentCaptureCompleted,
        ...outcome,
      },
      null,
      2,
    )}\n`,
    "utf8",
  );
  process.exitCode = passed ? 0 : 1;
} catch (error) {
  writeFileSync(
    outputPath,
    `${JSON.stringify({ status: "FAIL", harnessError: String(error) }, null, 2)}\n`,
    "utf8",
  );
  process.exitCode = 1;
} finally {
  clearInterval(keepAlive);
  socket?.close();
}
