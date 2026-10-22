// Per-user UI choice, independent of device settings and recording preferences.
export const OVERVIEW_HIDDEN_KEY = 'ol.overviewHidden.v1';
type OverviewStorage = Pick<Storage, 'getItem' | 'setItem'>;

export function readOverviewHidden(storage?: OverviewStorage): boolean {
  try {
    const target = storage ?? (typeof window === 'undefined' ? undefined : window.localStorage);
    return target?.getItem(OVERVIEW_HIDDEN_KEY) === '1';
  } catch { return false; }
}

export function rememberOverviewHidden(storage?: OverviewStorage): boolean {
  try {
    const target = storage ?? (typeof window === 'undefined' ? undefined : window.localStorage);
    if (!target) return false;
    target.setItem(OVERVIEW_HIDDEN_KEY, '1');
    return target.getItem(OVERVIEW_HIDDEN_KEY) === '1';
  } catch { return false; }
}

export function initialOverviewTab<T extends string>(requested: T, hidden: boolean): T | 'history' {
  return requested === 'overview' && hidden ? 'history' : requested;
}
