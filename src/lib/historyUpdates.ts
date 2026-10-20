import { isTauri } from './ipc';

export const HISTORY_UPDATED_EVENT = 'history:updated';

/** Listen to committed changes and recheck when returning from another app. */
export function subscribeHistoryUpdates(refresh: () => void): () => void {
  let disposed = false;
  let unlisten: (() => void) | undefined;
  const update = () => { if (!disposed) refresh(); };
  const onVisible = () => { if (document.visibilityState === 'visible') update(); };
  window.addEventListener(HISTORY_UPDATED_EVENT, update);
  window.addEventListener('focus', onVisible);
  document.addEventListener('visibilitychange', onVisible);
  if (isTauri) {
    void import('@tauri-apps/api/event').then(({ listen }) => listen(HISTORY_UPDATED_EVENT, update))
      .then(stop => {
        if (disposed) stop();
        else {
          unlisten = stop;
          // Close the gap between initial snapshot and async subscription.
          update();
        }
      }).catch(error => console.warn('[history] update subscription failed', error));
  }
  return () => {
    disposed = true;
    window.removeEventListener(HISTORY_UPDATED_EVENT, update);
    window.removeEventListener('focus', onVisible);
    document.removeEventListener('visibilitychange', onVisible);
    unlisten?.();
  };
}
