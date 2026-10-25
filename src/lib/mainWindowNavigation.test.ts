import assert from 'node:assert/strict';
import { createMainNavigationDrain, type MainWindowDestination } from './mainWindowNavigation.ts';

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
const flush = () => new Promise(resolve => setTimeout(resolve, 0));

{
  const first = deferred<MainWindowDestination | null>();
  const applied: MainWindowDestination[] = [];
  let reads = 0, active = 0, maxActive = 0;
  const drain = createMainNavigationDrain(async () => {
    active++; maxActive = Math.max(maxActive, active);
    const result = ++reads === 1 ? await first.promise : { kind: 'devicePage' as const, page: 'history' as const };
    active--; return result;
  }, destination => applied.push(destination), cause => { throw cause; });
  const pending = drain.request();
  void drain.request();
  first.resolve({ kind: 'providers' });
  await pending;
  assert.equal(maxActive, 1, 'notifications during an IPC read must not create overlapping reads');
  assert.equal(reads, 2, 'a notification during the first read must be drained afterwards');
  assert.deepEqual(applied, [{ kind: 'providers' }, { kind: 'devicePage', page: 'history' }]);
  drain.dispose();
}

{
  const first = deferred<MainWindowDestination | null>();
  let reads = 0;
  const errors: unknown[] = [], applied: MainWindowDestination[] = [];
  const drain = createMainNavigationDrain(() => ++reads === 1 ? first.promise : Promise.resolve({ kind: 'providers' }), destination => applied.push(destination), cause => errors.push(cause));
  const pending = drain.request();
  void drain.request();
  first.reject(new Error('controlled IPC failure'));
  await pending; await flush();
  assert.equal(errors.length, 1);
  assert.equal(reads, 2, 'a new request arriving during failure must still get its own attempt');
  assert.deepEqual(applied, [{ kind: 'providers' }]);
  drain.dispose();
}

{
  const first = deferred<MainWindowDestination | null>();
  let reads = 0;
  const applied: MainWindowDestination[] = [];
  const drain = createMainNavigationDrain(() => { reads++; return first.promise; }, destination => applied.push(destination), cause => { throw cause; });
  const pending = drain.request();
  void drain.request();
  drain.dispose();
  first.resolve({ kind: 'providers' });
  await pending;
  assert.deepEqual(applied, [], 'an unmounted renderer must not navigate on a late reply');
  assert.equal(reads, 1);
}
console.log('main navigation: serialized delivery, failure recovery and unmount checks passed');
