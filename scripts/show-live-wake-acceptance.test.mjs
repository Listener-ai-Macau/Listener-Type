#!/usr/bin/env node
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const source = readFileSync(new URL("./show-live-wake-acceptance.ps1", import.meta.url), "utf8");

test("popup captures explicit operator markers and preserves a missing candidate", () => {
  assert.match(source, /System\.Windows\.Forms/);
  assert.match(source, /event=start embedded_session_id=\(\\d\+\) origin=VoiceActivation/);
  assert.match(source, /associationTimeoutMs = \$AssociationTimeoutMs/);
  assert.match(source, /embeddedSessionId = \$script:roundCandidateId/);
  assert.match(source, /logByteOffset = \$script:roundLogOffset/);
  assert.match(source, /function Get-LatestFrontendCapsuleState/);
  assert.match(source, /if \(\$currentState -ne 'idle'\)/);
  assert.match(source, /正在等待 Listener 从 \$currentState 回到 idle/);
});

test("popup reads the live Listener log without fighting its writer", () => {
  assert.doesNotMatch(source, /ReadAllText/);
  assert.match(source, /FileShare\]::ReadWrite -bor \[System\.IO\.FileShare\]::Delete/);
  assert.match(source, /catch \[System\.IO\.IOException\]/);
  assert.match(source, /\$tailBytes = 256KB/);
  assert.match(source, /Read-AppendedLogText -Path \$resolvedLogPath -Offset \$tailOffset/);
  assert.match(source, /\$script:lastKnownCapsuleState = 'unknown'/);
  assert.match(source, /\$script:lastKnownCapsuleState -eq 'unknown' -and \$tailOffset -gt 0/);
  assert.match(source, /Read-AppendedLogText -Path \$resolvedLogPath -Offset 0/);
});

test("popup artifact contains no audio or transcript payload", () => {
  assert.doesNotMatch(source, /\.wav|transcriptText|audioData|pcmData/i);
  assert.match(source, /不保存音频或转写内容/);
  assert.match(source, /listener\.live-wake-attempt-markers\.v1/);
});
