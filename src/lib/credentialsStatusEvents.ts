// Configuration changed; listeners must read fresh status. Never send credential values.
export const CREDENTIALS_STATUS_CHANGED_EVENT = 'listener-type:credentials-status-changed';

export function notifyCredentialsStatusChanged(): void {
  if (typeof window !== 'undefined') {
    window.dispatchEvent(new Event(CREDENTIALS_STATUS_CHANGED_EVENT));
  }
}
