import type { CapsuleState } from './types';

export function capsuleCancelEnabled(state: CapsuleState): boolean {
  return state === 'recording'
    || state === 'transcribing'
    || state === 'polishing'
    || state === 'error';
}

export function capsuleConfirmEnabled(state: CapsuleState, stopPending: boolean): boolean {
  if (state === 'error') return true;
  return state === 'recording' && !stopPending;
}
