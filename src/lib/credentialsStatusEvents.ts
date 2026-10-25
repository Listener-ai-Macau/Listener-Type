// Configuration changed; listeners must read fresh status. Never send credential values.
export const CREDENTIALS_STATUS_CHANGED_EVENT = 'listener-type:credentials-status-changed';
const credentialsWindowId = Math.random().toString(36).slice(2);

if (typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window) {
  void import('@tauri-apps/api/event').then(({ listen }) =>
    listen<{ source: string }>(CREDENTIALS_STATUS_CHANGED_EVENT, event => {
      if (event.payload.source !== credentialsWindowId) {
        // Notify read-only consumers; never apply another window's credentials.
        window.dispatchEvent(new Event(CREDENTIALS_STATUS_CHANGED_EVENT));
      }
    }),
  ).catch(cause => console.warn('[credentials] status notification unavailable', cause));
}

export function notifyCredentialsStatusChanged(): void {
  if (typeof window !== 'undefined') {
    window.dispatchEvent(new Event(CREDENTIALS_STATUS_CHANGED_EVENT));
    if ('__TAURI_INTERNALS__' in window) {
      void import('@tauri-apps/api/event').then(({ emit }) =>
        emit(CREDENTIALS_STATUS_CHANGED_EVENT, { source: credentialsWindowId }),
      ).catch(cause => console.warn('[credentials] status notification failed', cause));
    }
  }
}
