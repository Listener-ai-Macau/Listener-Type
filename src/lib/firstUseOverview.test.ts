import assert from 'node:assert/strict';
import { OVERVIEW_HIDDEN_KEY, initialOverviewTab, readOverviewHidden, rememberOverviewHidden } from './firstUseOverview.ts';

const values = new Map<string, string>();
const storage = { getItem: (key: string) => values.get(key) ?? null,
  setItem: (key: string, value: string) => { values.set(key, value); } };
assert.equal(readOverviewHidden(storage), false);
assert.equal(initialOverviewTab('overview', false), 'overview');
assert.equal(rememberOverviewHidden(storage), true);
assert.equal(readOverviewHidden(storage), true);
assert.equal(initialOverviewTab('overview', readOverviewHidden(storage)), 'history');
for (const tab of ['history', 'tools', 'style', 'vocab', 'translation', 'selectionAsk']) {
  assert.equal(initialOverviewTab(tab, true), tab, 'explicit page destinations remain intact');
}
assert.deepEqual([...values.keys()], [OVERVIEW_HIDDEN_KEY], 'only this UI choice is written');
const denied = { getItem: () => { throw Error('denied'); }, setItem: () => { throw Error('denied'); } };
assert.equal(readOverviewHidden(denied), false);
assert.equal(rememberOverviewHidden(denied), false);
assert.equal(rememberOverviewHidden({ getItem: () => null, setItem: () => {} }), false,
  'a silently ignored write must not claim persistence');
