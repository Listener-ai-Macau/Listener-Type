import assert from 'node:assert/strict';
import {
  applyCapsulePayloadOrdering,
  createCapsuleOrderingTracker,
} from './capsuleEventOrdering.ts';
import type { CapsulePayload, CapsuleState } from './types.ts';

function payload(seq: number, sessionId: string, state: CapsuleState): CapsulePayload {
  return {
    seq,
    sessionId,
    state,
    level: 0,
    elapsedMs: 0,
    message: null,
    insertedChars: null,
    translation: false,
  };
}

{
  const tracker = createCapsuleOrderingTracker();
  assert.equal(applyCapsulePayloadOrdering(tracker, payload(1, 's1', 'recording')).accepted, true);
  assert.equal(applyCapsulePayloadOrdering(tracker, payload(2, 's1', 'cancelled')).accepted, true);
  assert.equal(applyCapsulePayloadOrdering(tracker, payload(3, 's1', 'idle')).accepted, true);

  const lateRecording = applyCapsulePayloadOrdering(tracker, payload(4, 's1', 'recording'));
  assert.equal(lateRecording.accepted, false);
  assert.equal(lateRecording.reason, 'closed-session-active-state');
}

{
  const tracker = createCapsuleOrderingTracker();
  assert.equal(applyCapsulePayloadOrdering(tracker, payload(10, 's1', 'recording')).accepted, true);

  const olderPartial = applyCapsulePayloadOrdering(tracker, payload(9, 's1', 'transcribing'));
  assert.equal(olderPartial.accepted, false);
  assert.equal(olderPartial.reason, 'older-sequence');
}

{
  const tracker = createCapsuleOrderingTracker();
  assert.equal(applyCapsulePayloadOrdering(tracker, payload(1, 's1', 'recording')).accepted, true);

  const staleIdle = applyCapsulePayloadOrdering(tracker, payload(2, 's0', 'idle'));
  assert.equal(staleIdle.accepted, false);
  assert.equal(staleIdle.reason, 'stale-idle-for-inactive-session');
}

{
  const tracker = createCapsuleOrderingTracker();
  assert.equal(applyCapsulePayloadOrdering(tracker, payload(1, 's1', 'recording')).accepted, true);
  assert.equal(applyCapsulePayloadOrdering(tracker, payload(2, 's1', 'idle')).accepted, true);
  assert.equal(applyCapsulePayloadOrdering(tracker, payload(3, 's2', 'recording')).accepted, true);
}

console.log('capsuleEventOrdering: all assertions passed');
