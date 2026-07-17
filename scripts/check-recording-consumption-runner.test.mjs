#!/usr/bin/env node
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import assert from "node:assert/strict";
import test from "node:test";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const runnerPath = join(scriptDir, "run-recording-consumption-physical-validation.ps1");
const source = readFileSync(runnerPath, "utf8");

function formalCaptureBlock() {
  const pattern =
    /\$captureProcess = Start-Process -FilePath "pwsh" -ArgumentList @\([\s\S]*?\) -WorkingDirectory \$firmwareRoot[\s\S]*?-WindowStyle Hidden/m;
  const match = pattern.exec(source);
  assert.notEqual(match, null, "missing formal serial capture Start-Process block");
  return match[0];
}

test("physical recording-consumption runner uses only installed Type and Type-owned artifacts", () => {
  assert.match(source, /\$installedType = "C:\\Program Files\\Listener Type\\listener-type\.exe"/);
  assert.match(source, /ExecutablePath -eq \$installedType/);
  assert.match(source, /Join-Path \$typeRoot "\.cache\\validation\\recording-consumption-20260717"/);
  assert.doesNotMatch(source, /Listener-Firmware\\cache/i);
});

test("physical recording-consumption runner locks COM3 with the canonical mutex", () => {
  assert.match(source, /New-Object System\.Threading\.Mutex\(\$false, "Global\\Listener_COM3"\)/);
  assert.match(source, /\$mutex\.WaitOne\(\[TimeSpan\]::FromSeconds\(8\)\)/);
  assert.match(source, /\$mutex\.ReleaseMutex\(\) \| Out-Null/);
});

test("formal serial capture does not pass CommandReadMs", () => {
  const formalCapture = formalCaptureBlock();
  assert.match(formalCapture, /"-CaptureSeconds"/);
  assert.match(formalCapture, /\$serialLog/);
  assert.doesNotMatch(formalCapture, /-CommandReadMs/);
});

test("preflight may use CommandReadMs only for device status", () => {
  assert.match(source, /-Command "~DEVICE:STATUS" -CommandReadMs 1800 -OutputPath \$statusLog/);
  assert.match(source, /-CaptureSeconds 12 -OutputPath \$readyLog/);
});

test("operator prompt is the canonical Chinese action gate", () => {
  assert.match(source, /"operator-prompt"/);
  assert.match(source, /"-ReviewStyle"/);
  assert.match(source, /"-Input"/);
  assert.match(source, /"-Json"/);
  assert.match(source, /"-SpokenText"/);
  assert.match(source, /"已完成,失败,中止"/);
});

test("runner waits for the capture deadline after the human prompt", () => {
  assert.match(source, /\$captureDeadlineUtc = \$captureStartTimeUtc\.AddSeconds\(\$CaptureSeconds \+ \$CaptureGraceSeconds\)/);
  assert.match(source, /\$waitAfterPromptMs = \[Math\]::Max\(\$minimumPostPromptWaitMs, \$remainingCaptureMs\)/);
});

test("runner invokes the recording-consumption machine checker", () => {
  assert.match(source, /check-recording-consumption-evidence\.mjs/);
  assert.match(source, /--serial-log \$serialLog/);
  assert.match(source, /--prompt-json \$promptResult/);
  assert.match(source, /--capture-start-iso \$captureStartIso/);
  assert.match(source, /--capture-end-iso \$captureEndIso/);
});
