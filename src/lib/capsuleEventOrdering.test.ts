import assert from 'node:assert/strict';
import {
  applyCapsulePayloadOrdering,
  createCapsuleOrderingTracker,
} from './capsuleEventOrdering.ts';
import type { CapsulePayload, CapsuleState } from './types.ts';

function payload(seq: number, sessionId: string | null, state: CapsuleState): CapsulePayload {
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

  const latePartial = applyCapsulePayloadOrdering(tracker, payload(5, 's1', 'transcribing'));
  assert.equal(latePartial.accepted, false);
  assert.equal(latePartial.reason, 'closed-session-active-state');
}

{
  const tracker = createCapsuleOrderingTracker();
  assert.equal(applyCapsulePayloadOrdering(tracker, payload(1, 's1', 'recording')).accepted, true);
  assert.equal(applyCapsulePayloadOrdering(tracker, payload(2, 's1', 'done')).accepted, true);

  const lateError = applyCapsulePayloadOrdering(tracker, payload(3, 's1', 'error'));
  assert.equal(lateError.accepted, false);
  assert.equal(lateError.reason, 'closed-session-error-state');
}

{
  const tracker = createCapsuleOrderingTracker();
  assert.equal(applyCapsulePayloadOrdering(tracker, payload(1, 's1', 'recording')).accepted, true);
  assert.equal(applyCapsulePayloadOrdering(tracker, payload(2, 's1', 'cancelled')).accepted, true);

  const lateError = applyCapsulePayloadOrdering(tracker, payload(3, 's1', 'error'));
  assert.equal(lateError.accepted, false);
  assert.equal(lateError.reason, 'closed-session-error-state');
}

{
  const tracker = createCapsuleOrderingTracker();
  assert.equal(applyCapsulePayloadOrdering(tracker, payload(1, 's1', 'recording')).accepted, true);
  assert.equal(applyCapsulePayloadOrdering(tracker, payload(2, 's1', 'error')).accepted, true);
  assert.equal(applyCapsulePayloadOrdering(tracker, payload(3, 's1', 'idle')).accepted, true);

  const lateError = applyCapsulePayloadOrdering(tracker, payload(4, 's1', 'error'));
  assert.equal(lateError.accepted, false);
  assert.equal(lateError.reason, 'closed-session-error-state');
}

{
  const tracker = createCapsuleOrderingTracker();
  assert.equal(applyCapsulePayloadOrdering(tracker, payload(1, 's1', 'recording')).accepted, true);
  assert.equal(applyCapsulePayloadOrdering(tracker, payload(2, 's1', 'cancelled')).accepted, true);

  const lateDone = applyCapsulePayloadOrdering(tracker, payload(3, 's1', 'done'));
  assert.equal(lateDone.accepted, false);
  assert.equal(lateDone.reason, 'closed-session-terminal-state');
}

{
  const tracker = createCapsuleOrderingTracker();
  assert.equal(applyCapsulePayloadOrdering(tracker, payload(1, 's1', 'recording')).accepted, true);
  assert.equal(applyCapsulePayloadOrdering(tracker, payload(2, 's1', 'done')).accepted, true);
  assert.equal(applyCapsulePayloadOrdering(tracker, payload(3, 's1', 'idle')).accepted, true);

  const lateCancelled = applyCapsulePayloadOrdering(tracker, payload(4, 's1', 'cancelled'));
  assert.equal(lateCancelled.accepted, false);
  assert.equal(lateCancelled.reason, 'closed-session-terminal-state');
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

  const staleOldRecording = applyCapsulePayloadOrdering(tracker, payload(4, 's1', 'recording'));
  assert.equal(staleOldRecording.accepted, false);
  assert.equal(staleOldRecording.reason, 'closed-session-active-state');
}

{
  const tracker = createCapsuleOrderingTracker();
  assert.equal(applyCapsulePayloadOrdering(tracker, payload(1, 's1', 'recording')).accepted, true);

  const nonSessionIdle = applyCapsulePayloadOrdering(tracker, payload(2, null, 'idle'));
  assert.equal(nonSessionIdle.accepted, false);
  assert.equal(nonSessionIdle.reason, 'non-session-terminal-while-session-active');
}

{
  const tracker = createCapsuleOrderingTracker();
  assert.equal(applyCapsulePayloadOrdering(tracker, payload(1, null, 'recording')).accepted, true);
  assert.equal(applyCapsulePayloadOrdering(tracker, payload(2, null, 'idle')).accepted, true);
}

console.log('capsuleEventOrdering: all assertions passed');
