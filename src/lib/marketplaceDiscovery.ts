import type { MarketplaceListItem, PolishMode, StylePack } from './types';

export type MarketplaceCategory = 'all' | PolishMode;

export const MARKETPLACE_PAGE_SIZE = 24;

export function marketplaceItemMatchesCategory(
  item: Pick<MarketplaceListItem, 'baseMode'>,
  category: MarketplaceCategory,
): boolean {
  return category === 'all' || item.baseMode === category;
}

export function mergeMarketplacePages(
  existing: MarketplaceListItem[],
  incoming: MarketplaceListItem[],
): MarketplaceListItem[] {
  const incomingById = new Map(incoming.map(item => [item.id, item]));
  const existingIds = new Set(existing.map(item => item.id));
  const merged = existing.map(item => incomingById.get(item.id) ?? item);
  for (const item of incoming) {
    if (!existingIds.has(item.id)) {
      merged.push(item);
    }
  }
  return merged;
}

export function buildInstalledMarketplaceIds(packs: StylePack[]): Set<string> {
  const ids = new Set<string>();
  for (const pack of packs) {
    if (pack.id.trim().length > 0) {
      ids.add(pack.id);
    }
    const originPackId = pack.originPackId?.trim();
    if (originPackId) {
      ids.add(originPackId);
    }
  }
  return ids;
}

export function inferMarketplaceNextOffset(input: {
  offset: number;
  itemCount: number;
  nextOffset: number | null | undefined;
  hasMore: boolean;
}): number | null {
  if (!input.hasMore) return null;
  if (typeof input.nextOffset === 'number' && Number.isFinite(input.nextOffset) && input.nextOffset >= 0) {
    return input.nextOffset;
  }
  return input.offset + input.itemCount;
}
