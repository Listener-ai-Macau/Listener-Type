import assert from 'node:assert/strict';
import { newerNetworkHealth, networkHealthPresentation, type NetworkHealthView } from './networkHealth.ts';

assert.deepEqual(networkHealthPresentation(null), { tone: 'default', key: 'unknown' }, 'not observed cannot mean available');
const failure: NetworkHealthView = { seq: 12, class: 'asr_unreachable' };
const recovered: NetworkHealthView = { seq: 13, class: 'all_good' };
let current = newerNetworkHealth(null, failure);
assert.equal(networkHealthPresentation(current).key, 'recognitionUnavailable');
current = newerNetworkHealth(current, recovered);
assert.equal(networkHealthPresentation(current).key, 'available');
assert.equal(newerNetworkHealth(current, failure), recovered, 'late snapshots cannot restore an old failure');
assert.equal(newerNetworkHealth(current, { seq: 13, class: 'all_bad' }), recovered, 'a duplicate revision cannot overwrite recovery');
assert.equal(newerNetworkHealth(current, { seq: NaN, class: 'all_bad' }), recovered);
assert.equal(newerNetworkHealth(null, { seq: 0, class: 'all_good' })?.seq, 0);
assert.equal(networkHealthPresentation({ seq: 14, class: 'general_unreachable' }).key, 'recognitionAvailable', 'failed control site does not invalidate ASR');
const quota = newerNetworkHealth(current, { seq: 14, class: 'all_good', providerQuotaExceeded: true });
assert.equal(networkHealthPresentation(quota).key, 'quota', 'reachability is not a usable recognition allowance');
assert.equal(networkHealthPresentation(newerNetworkHealth(quota, { seq: 15, class: 'all_good', providerQuotaExceeded: false })).key, 'available');
console.log('network health display: unknown, ordered failure/recovery, independent probe and quota boundaries passed');
