// Observe existing writes without queuing them, retaining values or changing provider scope.
// A nested settings window must finish its writes before refreshing the underlying trial.
const pending = new Set<Promise<unknown>>();

export async function trackProviderWrite<T>(write: () => Promise<T>): Promise<T> {
  const operation = write();
  pending.add(operation);
  try { return await operation; }
  finally { pending.delete(operation); }
}

export async function waitForProviderWrites(): Promise<boolean> {
  let succeeded = true;
  while (pending.size) {
    const results = await Promise.allSettled([...pending]);
    if (results.some(result => result.status === 'rejected')) succeeded = false;
  }
  return succeeded;
}
