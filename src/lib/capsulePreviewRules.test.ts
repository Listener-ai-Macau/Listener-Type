import { truncatePreview, PREVIEW_MAX_CHARS, PREVIEW_FINAL_TRANSITION, LAYOUT_RULES } from './capsulePreviewRules.ts';

// Truncate under max chars — returns as-is
console.assert(
  truncatePreview('你好', 'win', 'default') === '你好',
  'short text should pass through',
);

// Truncate long text — tail with ellipsis
{
  const long = '这是一段很长的测试文本需要被截断处理才能放进胶囊';
  const result = truncatePreview(long, 'win', 'processing');
  console.assert(result.startsWith('...'), 'should start with ...');
  const chars = Array.from(result.slice(3));
  console.assert(chars.length === PREVIEW_MAX_CHARS.win.processing, 'should truncate to max chars');
}

// Whitespace normalization
console.assert(
  truncatePreview('  hello   world  ', 'win', 'default') === 'hello world',
  'should normalize whitespace',
);

// Mac limits
{
  const text = '这段文本比较长需要截断处理一下';
  const result = truncatePreview(text, 'mac', 'default');
  console.assert(result.startsWith('...'), 'mac should also truncate');
  const chars = Array.from(result.slice(3));
  console.assert(chars.length === PREVIEW_MAX_CHARS.mac.default, 'should use mac limit');
}

// Preview states include recording/transcribing/polishing
console.assert(
  PREVIEW_FINAL_TRANSITION.previewStates.includes('recording'),
  'recording should be a preview state',
);
console.assert(
  PREVIEW_FINAL_TRANSITION.finalState === 'done',
  'final state should be done',
);
console.assert(
  PREVIEW_FINAL_TRANSITION.stopAckMs >= 400 && PREVIEW_FINAL_TRANSITION.stopAckMs <= 900,
  'stop acknowledgement should be perceptible but brief',
);
console.assert(
  PREVIEW_FINAL_TRANSITION.exitAnimMs < 200,
  'exit animation should be fast',
);

// Layout heights are positive
console.assert(LAYOUT_RULES.fixedHeight.win > 0, 'win height should be positive');
console.assert(LAYOUT_RULES.fixedHeight.mac > 0, 'mac height should be positive');

console.log('capsulePreviewRules: all assertions passed');
