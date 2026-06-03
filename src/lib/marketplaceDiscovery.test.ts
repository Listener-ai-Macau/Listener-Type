import assert from 'node:assert/strict';
import {
  buildInstalledMarketplaceIds,
  inferMarketplaceNextOffset,
  marketplaceItemMatchesCategory,
  mergeMarketplacePages,
} from './marketplaceDiscovery.ts';
import type { MarketplaceListItem, StylePack } from './types.ts';

function item(id: string, overrides: Partial<MarketplaceListItem> = {}): MarketplaceListItem {
  return {
    id,
    slug: id,
    name: id,
    description: '',
    authorLogin: 'alice',
    version: '1.0.0',
    baseMode: 'structured',
    tags: [],
    likeCount: 0,
    downloadCount: 0,
    publishedAt: '2026-06-01T00:00:00Z',
    updatedAt: '2026-06-01T00:00:00Z',
    originPackId: null,
    originAuthorLogin: null,
    ...overrides,
  };
}

function pack(id: string, originPackId?: string | null): StylePack {
  return {
    id,
    name: id,
    description: '',
    author: 'alice',
    version: '1.0.0',
    kind: 'user',
    baseMode: 'structured',
    prompt: '',
    examples: [],
    tags: [],
    iconPath: null,
    createdAt: '2026-06-01T00:00:00Z',
    updatedAt: '2026-06-01T00:00:00Z',
    enabled: true,
    active: false,
    recommendedModel: null,
    compatibleAppVersion: null,
    originPackId,
    originAuthorLogin: null,
  };
}

assert.equal(marketplaceItemMatchesCategory(item('a', { baseMode: 'raw' }), 'all'), true);
assert.equal(marketplaceItemMatchesCategory(item('a', { baseMode: 'raw' }), 'raw'), true);
assert.equal(marketplaceItemMatchesCategory(item('a', { baseMode: 'raw' }), 'formal'), false);

assert.deepEqual(
  mergeMarketplacePages(
    [item('a'), item('b', { likeCount: 1 })],
    [item('b', { likeCount: 9 }), item('c')],
  ).map(value => [value.id, value.likeCount]),
  [['a', 0], ['b', 9], ['c', 0]],
);

const installed = buildInstalledMarketplaceIds([
  pack('local-only', null),
  pack('local-installed', 'remote-pack'),
]);
assert.equal(installed.has('local-only'), true);
assert.equal(installed.has('remote-pack'), true);
assert.equal(installed.has('missing'), false);

assert.equal(inferMarketplaceNextOffset({ offset: 24, itemCount: 24, nextOffset: 72, hasMore: true }), 72);
assert.equal(inferMarketplaceNextOffset({ offset: 24, itemCount: 24, nextOffset: null, hasMore: true }), 48);
assert.equal(inferMarketplaceNextOffset({ offset: 24, itemCount: 24, nextOffset: 48, hasMore: false }), null);
