import {
  truncatePreview,
  PREVIEW_MAX_CHARS,
  PREVIEW_FINAL_TRANSITION,
  LAYOUT_RULES,
  shouldShowStopAcknowledgement,
} from './capsulePreviewRules.ts';

function assertEqual(actual: unknown, expected: unknown, name: string) {
  if (actual !== expected) {
    throw new Error(`${name}: expected ${String(expected)}, got ${String(actual)}`);
  }
}

function assertOk(value: unknown, name: string) {
  if (!value) {
    throw new Error(`${name}: expected truthy value`);
  }
}

// Truncate under max chars — returns as-is
assertEqual(
  truncatePreview('你好', 'win', 'default'),
  '你好',
  'short text should pass through',
);

// Truncate long text — tail with ellipsis
{
  const long = '这是一段很长的测试文本需要被截断处理才能放进胶囊里面显示给用户看的预览内容区域';
  const result = truncatePreview(long, 'win', 'processing');
  assertOk(result.startsWith('...'), 'should start with ...');
  const chars = Array.from(result.slice(3));
  assertEqual(chars.length, PREVIEW_MAX_CHARS.win.processing, 'should truncate to max chars');
  assertEqual(
    PREVIEW_MAX_CHARS.win.processing,
    28,
    'win processing preview must fit its two-line capsule column with the ellipsis',
  );
}

// Recording owns the full two-line column, unlike processing which reserves
// room for the spinner. A longer tail window keeps CJK rows visually balanced.
{
  const long = '这是一段很长的测试文本需要被截断处理才能放进胶囊里面显示给用户看的录音预览内容区域';
  const result = truncatePreview(long, 'win', 'recording');
  assertOk(result.startsWith('...'), 'recording preview should preserve the tail marker');
  assertEqual(
    Array.from(result.slice(3)).length,
    32,
    'win recording preview should fill its two-line text column more evenly',
  );
}

// Whitespace normalization
assertEqual(
  truncatePreview('  hello   world  ', 'win', 'default'),
  'hello world',
  'should normalize whitespace',
);

// Mac limits
{
  const text = '这段文本比较长需要截断处理一下';
  const result = truncatePreview(text, 'mac', 'default');
  assertOk(result.startsWith('...'), 'mac should also truncate');
  const chars = Array.from(result.slice(3));
  assertEqual(chars.length, PREVIEW_MAX_CHARS.mac.default, 'should use mac limit');
}

// Preview states include recording/transcribing/polishing
assertOk(
  PREVIEW_FINAL_TRANSITION.previewStates.includes('recording'),
  'recording should be a preview state',
);
assertEqual(
  PREVIEW_FINAL_TRANSITION.finalState,
  'done',
  'final state should be done',
);
assertOk(
  PREVIEW_FINAL_TRANSITION.stopAckMs >= 400 && PREVIEW_FINAL_TRANSITION.stopAckMs <= 900,
  'stop acknowledgement should be perceptible but brief',
);
assertOk(
  shouldShowStopAcknowledgement('recording', true),
  'stop acknowledgement should appear immediately while recording after stop is requested',
);
assertOk(
  shouldShowStopAcknowledgement('transcribing', true),
  'stop acknowledgement should remain visible through transcribing',
);
assertOk(
  shouldShowStopAcknowledgement('polishing', true),
  'stop acknowledgement should remain visible through polishing',
);
assertOk(
  !shouldShowStopAcknowledgement('done', true),
  'stop acknowledgement should clear once final feedback is shown',
);
assertOk(
  !shouldShowStopAcknowledgement('recording', false),
  'inactive stop acknowledgement should stay hidden',
);
assertOk(
  PREVIEW_FINAL_TRANSITION.exitAnimMs < 200,
  'exit animation should be fast',
);

// Layout heights are positive
assertOk(LAYOUT_RULES.fixedHeight.win > 0, 'win height should be positive');
assertOk(LAYOUT_RULES.fixedHeight.mac > 0, 'mac height should be positive');

console.log('capsulePreviewRules: all assertions passed');
