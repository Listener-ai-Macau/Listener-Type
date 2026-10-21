// Observe existing writes without queuing them, retaining values or changing provider scope.
// A nested settings window must finish its writes before refreshing the underlying trial.
const pending = new Map<Promise<unknown>, number>();
const latest = new Map<string, { revision: number; failed: boolean }>();
let revision = 0;

export function providerWriteRevision(): number { return revision; }

export async function trackProviderWrite<T>(scope: string, write: () => Promise<T>): Promise<T> {
  const ownRevision = ++revision;
  const record = (failed: boolean) => {
    if ((latest.get(scope)?.revision ?? -1) < ownRevision) {
      latest.set(scope, { revision: ownRevision, failed });
    }
  };
  // Start immediately, while converting a synchronous throw into an observed rejection.
  const operation = (async () => write())();
  pending.set(operation, ownRevision);
  try { const result = await operation; record(false); return result; }
  catch (error) { record(true); throw error; }
  finally { pending.delete(operation); }
}

export async function waitForProviderWrites(since: number): Promise<boolean> {
  for (;;) {
    const current = [...pending].filter(([, version]) => version > since).map(([operation]) => operation);
    if (!current.length) break;
    await Promise.allSettled(current);
  }
  // A failed save may have finished before Close. Keep that outcome until a
  // newer successful write for the same field/switch, not an unrelated success.
  return ![...latest.values()].some(result => result.revision > since && result.failed);
}
