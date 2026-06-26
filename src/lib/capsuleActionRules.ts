import type { CapsuleState } from './types';

export function capsuleCancelEnabled(state: CapsuleState): boolean {
  return state !== 'idle';
}

export function capsuleConfirmEnabled(state: CapsuleState, stopPending: boolean): boolean {
  if (state === 'error') return true;
  return state === 'recording' && !stopPending;
}
