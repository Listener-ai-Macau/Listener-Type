export const OPEN_DEMO_MODE_EVENT = 'listener-type:open-demo-mode';

const DEMO_MODE_PENDING_KEY = 'listener-type:pending-demo-mode';

export function requestDemoMode(): void {
  window.sessionStorage.setItem(DEMO_MODE_PENDING_KEY, '1');
  window.dispatchEvent(new CustomEvent(OPEN_DEMO_MODE_EVENT));
}

export function consumePendingDemoMode(): boolean {
  const pending = window.sessionStorage.getItem(DEMO_MODE_PENDING_KEY) === '1';
  if (pending) {
    window.sessionStorage.removeItem(DEMO_MODE_PENDING_KEY);
  }
  return pending;
}
