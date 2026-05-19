import assert from 'node:assert/strict';
import { truncatePreview, PREVIEW_MAX_CHARS, PREVIEW_FINAL_TRANSITION, LAYOUT_RULES } from './capsulePreviewRules.ts';

// Truncate under max chars — returns as-is
assert.strictEqual(
  truncatePreview('你好', 'win', 'default'),
  '你好',
  'short text should pass through',
);

// Truncate long text — tail with ellipsis
{
  const long = '这是一段很长的测试文本需要被截断处理才能放进胶囊里面显示给用户看的预览内容区域';
  const result = truncatePreview(long, 'win', 'processing');
  assert.ok(result.startsWith('...'), 'should start with ...');
  const chars = Array.from(result.slice(3));
  assert.strictEqual(chars.length, PREVIEW_MAX_CHARS.win.processing, 'should truncate to max chars');
}

// Whitespace normalization
assert.strictEqual(
  truncatePreview('  hello   world  ', 'win', 'default'),
  'hello world',
  'should normalize whitespace',
);

// Mac limits
{
  const text = '这段文本比较长需要截断处理一下';
  const result = truncatePreview(text, 'mac', 'default');
  assert.ok(result.startsWith('...'), 'mac should also truncate');
  const chars = Array.from(result.slice(3));
  assert.strictEqual(chars.length, PREVIEW_MAX_CHARS.mac.default, 'should use mac limit');
}

// Preview states include recording/transcribing/polishing
assert.ok(
  PREVIEW_FINAL_TRANSITION.previewStates.includes('recording'),
  'recording should be a preview state',
);
assert.strictEqual(
  PREVIEW_FINAL_TRANSITION.finalState,
  'done',
  'final state should be done',
);
assert.ok(
  PREVIEW_FINAL_TRANSITION.stopAckMs >= 400 && PREVIEW_FINAL_TRANSITION.stopAckMs <= 900,
  'stop acknowledgement should be perceptible but brief',
);
assert.ok(
  PREVIEW_FINAL_TRANSITION.exitAnimMs < 200,
  'exit animation should be fast',
);

// Layout heights are positive
assert.ok(LAYOUT_RULES.fixedHeight.win > 0, 'win height should be positive');
assert.ok(LAYOUT_RULES.fixedHeight.mac > 0, 'mac height should be positive');

console.log('capsulePreviewRules: all assertions passed');
