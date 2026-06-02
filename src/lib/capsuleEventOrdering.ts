import type { CapsulePayload, CapsuleState } from './types';

export interface CapsuleOrderingTracker {
  lastSeq: number;
  activeSessionId: string | null;
  closedSessionIds: Set<string>;
}

export interface CapsuleOrderingDecision {
  accepted: boolean;
  reason?: string;
}

const ACTIVE_STATES: ReadonlySet<CapsuleState> = new Set(['recording', 'transcribing', 'polishing']);
const TERMINAL_STATES: ReadonlySet<CapsuleState> = new Set(['idle', 'done', 'cancelled', 'error']);

export function createCapsuleOrderingTracker(): CapsuleOrderingTracker {
  return {
    lastSeq: 0,
    activeSessionId: null,
    closedSessionIds: new Set<string>(),
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

  if (sessionId && ACTIVE_STATES.has(payload.state) && tracker.closedSessionIds.has(sessionId)) {
    return { accepted: false, reason: 'closed-session-active-state' };
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
    tracker.closedSessionIds.add(sessionId);
    if (tracker.activeSessionId === sessionId) {
      tracker.activeSessionId = null;
    }
  }

  return { accepted: true };
}
