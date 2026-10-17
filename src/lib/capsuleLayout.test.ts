import assert from 'node:assert/strict';
import {
  getCapsulePillMetrics,
  getCapsuleHostMetrics,
  getCapsuleMessageLayout,
  CAPSULE_BADGE_HEIGHT,
  CAPSULE_BADGE_STACK_STEP,
} from './capsuleLayout.ts';

// ── Pill metrics ──────────────────────────────────────────────

// Windows pill
{
  const m = getCapsulePillMetrics('win');
  assert.strictEqual(m.width, 280, 'win pill width');
  assert.strictEqual(m.height, 52, 'win pill height');
  assert.strictEqual(m.textWidth, 192, 'win text width');
  assert.strictEqual(m.boxSizing, 'border-box', 'win box-sizing');
  assert.ok(m.textWidth < m.width, 'text area smaller than pill');
}

// macOS pill
{
  const m = getCapsulePillMetrics('mac');
  assert.strictEqual(m.width, 176, 'mac pill width');
  assert.strictEqual(m.height, 42, 'mac pill height');
  assert.strictEqual(m.textWidth, 84, 'mac text width');
}

// ── Host metrics ──────────────────────────────────────────────

// Windows without translation
{
  const h = getCapsuleHostMetrics('win', false);
  assert.strictEqual(h.width, 304, 'win host width = pill(280) + insets(24)');
  assert.strictEqual(h.height, 105, 'win host includes the complete network badge');
  assert.strictEqual(h.horizontalInset, 12, 'win horizontal inset');
  assert.strictEqual(h.bottomInset, 12, 'win bottom inset');
  assert.strictEqual(
    h.width, getCapsulePillMetrics('win').width + h.horizontalInset * 2,
    'host width = pill width + 2 * inset',
  );
}

// Windows with translation
{
  const h = getCapsuleHostMetrics('win', true);
  assert.strictEqual(h.width, 304, 'translation keeps same width');
  assert.strictEqual(h.height, 131, 'translation grows upward by one badge row');
}

// The reported network badge was only given 12px of a 21px box in the old
// 84px host. Fit both badges and their shadows without depending on font metrics.
for (const translation of [false, true]) {
  const host = getCapsuleHostMetrics('win', translation);
  const pill = getCapsulePillMetrics('win');
  const lowerBadgeBottom = host.bottomInset + pill.height + host.badgeGap;
  const networkBottom = lowerBadgeBottom + (translation ? CAPSULE_BADGE_STACK_STEP : 0);
  assert.ok(host.height - networkBottom - CAPSULE_BADGE_HEIGHT >= 12,
    'the complete upper badge and shadow fit inside the native window');
  if (translation) {
    assert.ok(networkBottom >= lowerBadgeBottom + CAPSULE_BADGE_HEIGHT + 5,
      'network and translation rows have their own non-overlapping space');
  }
}

// macOS host
{
  const h = getCapsuleHostMetrics('mac', false);
  assert.strictEqual(h.width, 176, 'mac host width');
  assert.strictEqual(h.height, 42, 'mac host height');
  assert.strictEqual(h.horizontalInset, 0, 'mac no inset');
}

// ── Message layout ────────────────────────────────────────────

// Windows processing/error allows 2-line wrap
{
  const lp = getCapsuleMessageLayout('win', 'processing');
  assert.strictEqual(lp.allowWrap, true, 'win processing wraps');
  assert.strictEqual(lp.lineClamp, 2, 'win processing 2 lines');

  const le = getCapsuleMessageLayout('win', 'error');
  assert.strictEqual(le.allowWrap, true, 'win error wraps');
  assert.strictEqual(le.lineClamp, 2, 'win error 2 lines');
}

// Windows default is single line
{
  const l = getCapsuleMessageLayout('win', 'default');
  assert.strictEqual(l.allowWrap, false, 'win default no wrap');
  assert.strictEqual(l.lineClamp, 1, 'win default 1 line');
}

// macOS never wraps
{
  const lp = getCapsuleMessageLayout('mac', 'processing');
  assert.strictEqual(lp.allowWrap, false, 'mac processing no wrap');
  assert.strictEqual(lp.lineClamp, 1, 'mac always 1 line');

  const le = getCapsuleMessageLayout('mac', 'error');
  assert.strictEqual(le.allowWrap, false, 'mac error no wrap');
}

console.log('capsuleLayout: all assertions passed');
