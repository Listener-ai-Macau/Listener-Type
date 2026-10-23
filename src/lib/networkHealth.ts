/** Ordered display-only observations from the existing native reachability owner. */
export interface NetworkHealthView {
  seq: number;
  class: 'all_good' | 'asr_unreachable' | 'general_unreachable' | 'all_bad';
  providerQuotaExceeded?: boolean;
}

export function newerNetworkHealth(current: NetworkHealthView | null, next: NetworkHealthView): NetworkHealthView | null {
  if (!Number.isSafeInteger(next.seq) || next.seq < 0 || (current && next.seq <= current.seq)) return current;
  if (!['all_good', 'asr_unreachable', 'general_unreachable', 'all_bad'].includes(next.class)) return current;
  return next;
}

export function networkHealthPresentation(view: NetworkHealthView | null): { tone: 'default' | 'ok' | 'outline'; key: string } {
  if (!view) return { tone: 'default', key: 'unknown' };
  if (view.providerQuotaExceeded) return { tone: 'outline', key: 'quota' };
  switch (view.class) {
    case 'all_good': return { tone: 'ok', key: 'available' };
    case 'general_unreachable': return { tone: 'ok', key: 'recognitionAvailable' };
    case 'asr_unreachable': return { tone: 'outline', key: 'recognitionUnavailable' };
    case 'all_bad': return { tone: 'outline', key: 'unavailable' };
  }
}
