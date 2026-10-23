// Serialize explicit edits from the settings and key cards, including their
// fresh read + compare-and-set. A failed save must not poison later retries.
let previous: Promise<unknown> = Promise.resolve();
export function serializeDeviceSettingsSave<T>(save: () => Promise<T>): Promise<T> {
  const next = previous.then(save, save);
  previous = next.catch(() => undefined);
  return next;
}
