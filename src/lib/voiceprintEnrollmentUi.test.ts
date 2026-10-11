import assert from 'node:assert/strict';
import { voiceprintFailureUi } from './voiceprintEnrollmentUi.ts';

const storage = voiceprintFailureUi({ failureStage: 'storage', canRetrySave: true });
assert.equal(storage.retry, 'save', 'accepted samples must retry storage without asking for new audio');
assert.equal(storage.sampleError, false, 'a vault error must not blame microphone input');
assert.equal(voiceprintFailureUi({ failureStage: 'storage', canRetrySave: false }).retry, 'none');
for (const failureStage of ['capture', 'phrase'] as const) {
  const failure = voiceprintFailureUi({ failureStage, canRetrySave: false });
  assert.equal(failure.retry, 'record');
  assert.equal(failure.sampleError, true);
}
for (const failureStage of ['device', 'preparation', 'features', null] as const) {
  assert.equal(voiceprintFailureUi({ failureStage, canRetrySave: false }).sampleError, false);
}
assert.equal(voiceprintFailureUi(null).sampleError, false, 'missing evidence must not imply bad speech');
console.log('Voice enrollment failure actions passed');
