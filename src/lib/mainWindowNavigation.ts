import type { DeviceCustomKeyAppPage } from './types';

export type MainWindowDestination =
  | { kind: 'providers' }
  | { kind: 'devicePage'; page: DeviceCustomKeyAppPage };

// Serialize reads as well as applying them: two overlapping IPC replies must
// not restore an older page after a newer request has already been applied.
export function createMainNavigationDrain(
  read: () => Promise<MainWindowDestination | null>,
  apply: (destination: MainWindowDestination) => void,
  onError: (cause: unknown) => void,
) {
  let disposed = false;
  let pending = false;
  let draining = false;
  const request = async (): Promise<void> => {
    if (disposed) return;
    pending = true;
    if (draining) return;
    draining = true;
    try {
      while (pending && !disposed) {
        pending = false;
        const destination = await read();
        if (!disposed && destination) apply(destination);
      }
    } catch (cause) {
      if (!disposed) onError(cause);
    } finally {
      draining = false;
      if (pending && !disposed) void request();
    }
  };
  return { request, dispose: () => { disposed = true; pending = false; } };
}
