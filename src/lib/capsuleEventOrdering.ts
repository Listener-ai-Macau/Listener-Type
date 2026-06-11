import type { CapsulePayload, CapsuleState } from './types';

export interface CapsuleOrderingTracker {
  lastSeq: number;
  activeSessionId: string | null;
  closedSessionStates: Map<string, CapsuleState>;
}

export interface CapsuleOrderingDecision {
  accepted: boolean;
  reason?: string;
}

const ACTIVE_STATES: ReadonlySet<CapsuleState> = new Set(['reconnecting', 'recording', 'transcribing', 'polishing']);
const TERMINAL_STATES: ReadonlySet<CapsuleState> = new Set(['idle', 'done', 'cancelled', 'error']);

export function createCapsuleOrderingTracker(): CapsuleOrderingTracker {
  return {
    lastSeq: 0,
    activeSessionId: null,
    closedSessionStates: new Map<string, CapsuleState>(),
  };
}

export function applyCapsulePayloadOrdering(
  tracker: CapsuleOrderingTracker,
  payload: CapsulePayload,
): CapsuleOrderingDecision {
  const seq = Number.isFinite(payload.seq) ? payload.seq : 0;
  const sessionId = payload.sessionId ?? null;

  if (seq > 0 && seq <= tracker.lastSeq) {
    return { accepted: false, reason: 'older-sequence' };
  }

  if (!sessionId && tracker.activeSessionId && ACTIVE_STATES.has(payload.state)) {
    return { accepted: false, reason: 'non-session-active-while-session-active' };
  }

  if (!sessionId && tracker.activeSessionId && TERMINAL_STATES.has(payload.state)) {
    return { accepted: false, reason: 'non-session-terminal-while-session-active' };
  }

  const closedState = sessionId ? tracker.closedSessionStates.get(sessionId) : undefined;

  if (sessionId && ACTIVE_STATES.has(payload.state) && closedState) {
    return { accepted: false, reason: 'closed-session-active-state' };
  }

  if (
    sessionId
    && payload.state === 'error'
    && closedState
    && closedState !== 'error'
  ) {
    return { accepted: false, reason: 'closed-session-error-state' };
  }

  if (
    sessionId
    && closedState
    && TERMINAL_STATES.has(payload.state)
    && payload.state !== 'idle'
    && closedState !== payload.state
  ) {
    return { accepted: false, reason: 'closed-session-terminal-state' };
  }

  if (
    sessionId &&
    payload.state === 'idle' &&
    tracker.activeSessionId &&
    tracker.activeSessionId !== sessionId
  ) {
    return { accepted: false, reason: 'stale-idle-for-inactive-session' };
  }

  if (seq > 0) {
    tracker.lastSeq = seq;
  }
  if (sessionId && ACTIVE_STATES.has(payload.state)) {
    tracker.activeSessionId = sessionId;
  }
  if (sessionId && TERMINAL_STATES.has(payload.state)) {
    tracker.closedSessionStates.set(sessionId, payload.state);
    if (tracker.activeSessionId === sessionId) {
      tracker.activeSessionId = null;
    }
  }

  return { accepted: true };
}
