import assert from 'node:assert/strict';
import { capsuleCancelEnabled, capsuleConfirmEnabled } from './capsuleActionRules.ts';
import type { CapsuleState } from './types.ts';

const activeStates: CapsuleState[] = ['recording', 'transcribing', 'polishing'];

for (const state of activeStates) {
  assert.equal(capsuleCancelEnabled(state), true, `${state} should allow cancelling`);
}

assert.equal(capsuleCancelEnabled('error'), true, 'error should allow dismiss');
assert.equal(capsuleCancelEnabled('done'), false, 'done should not expose cancel action');
assert.equal(capsuleCancelEnabled('cancelled'), false, 'cancelled should not expose cancel action');
assert.equal(capsuleCancelEnabled('idle'), false, 'idle should not expose cancel action');
assert.equal(capsuleCancelEnabled('reconnecting'), false, 'reconnecting should not expose cancel action');

assert.equal(capsuleConfirmEnabled('recording', false), true, 'recording should allow stop');
assert.equal(capsuleConfirmEnabled('recording', true), false, 'stop pending should not allow repeated stop');
assert.equal(capsuleConfirmEnabled('reconnecting', false), false, 'reconnecting should not allow confirm');
assert.equal(capsuleConfirmEnabled('transcribing', false), false, 'transcribing should not allow confirm');
assert.equal(capsuleConfirmEnabled('polishing', false), false, 'polishing should not allow confirm');
assert.equal(capsuleConfirmEnabled('error', false), true, 'error confirm retries');

console.log('capsuleActionRules: all assertions passed');
