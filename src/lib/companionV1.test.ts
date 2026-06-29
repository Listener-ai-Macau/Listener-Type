import assert from 'node:assert/strict';
import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname } from 'node:path';
import {
  COMPANION_V1_GATT_BOUNDARY,
  applyCompanionV1ControlFixture,
  companionBleNameIsValid,
  createCompanionV1Fixture,
} from './companionV1.ts';

assert.equal(companionBleNameIsValid('companion-v1'), true);
assert.equal(companionBleNameIsValid('companion lab'), true);
assert.equal(companionBleNameIsValid(''), false);
assert.equal(companionBleNameIsValid('companion=lab'), false);
assert.equal(companionBleNameIsValid('companion-123456789012345678901'), false);

const initial = createCompanionV1Fixture();
assert.equal(initial.schema, 'companion.host.v1');
assert.equal(initial.meeting.state, 'idle');
assert.equal(initial.gatt.meetingControlUuid, COMPANION_V1_GATT_BOUNDARY.meetingControlUuid);

const named = applyCompanionV1ControlFixture(initial, {
  action: 'stageBleName',
  requestId: 9,
  bleName: 'companion-v1',
});
assert.equal(named.bleName.stagedName, 'companion-v1');
assert.equal(named.bleName.pendingRestart, true);
assert.equal(named.lastControl?.characteristicUuid, COMPANION_V1_GATT_BOUNDARY.bleNameConfigUuid);

const finalized = applyCompanionV1ControlFixture(initial, {
  action: 'meetingFinalize',
  requestId: 11,
  maxSeconds: 3600,
  profileId: 2,
});
assert.equal(finalized.meeting.state, 'finalized');
assert.equal(finalized.meeting.syncState, 'readyForHostSync');
assert.equal(finalized.lastControl?.characteristicUuid, COMPANION_V1_GATT_BOUNDARY.meetingControlUuid);

const wakeDetected = applyCompanionV1ControlFixture(initial, {
  action: 'wakeTestDetect',
  requestId: 12,
  sensitivity: 64,
});
assert.equal(wakeDetected.wakeWord.lastEvent, 'detected');
assert.equal(wakeDetected.wakeWord.detectionCount, 1);
assert.equal(wakeDetected.lastControl?.characteristicUuid, COMPANION_V1_GATT_BOUNDARY.wakeWordControlUuid);

assert.throws(
  () => applyCompanionV1ControlFixture(initial, { action: 'stageBleName', bleName: 'bad=name' }),
  /invalidCompanionBleName/,
);

if (process.argv.includes('--write-docs')) {
  const artifact = {
    schema: 'listener_type.companion_v1_fixture.v1',
    generatedAt: 'fixture-deterministic',
    noHardwareRequired: true,
    hostSurface: {
      panel: 'settings.device.companionV1',
      controls: [
        'stageBleName',
        'applyBleName',
        'resetBleName',
        'meetingStart',
        'meetingStop',
        'meetingFinalize',
        'speakerPrompt',
        'speakerStop',
        'wakeArm',
        'wakeDisable',
        'wakeTestDetect',
      ],
    },
    snapshots: {
      initial,
      named,
      finalized,
      wakeDetected,
    },
  };
  const outputPath = 'docs/validation/companion_v1_type_surface.json';
  mkdirSync(dirname(outputPath), { recursive: true });
  writeFileSync(outputPath, `${JSON.stringify(artifact, null, 2)}\n`, 'utf8');
  console.log(`companionV1: wrote ${outputPath}`);
} else {
  console.log('companionV1: all assertions passed');
}
