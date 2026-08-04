import {
  truncatePreview,
  PREVIEW_MAX_CHARS,
  PREVIEW_FINAL_TRANSITION,
  LAYOUT_RULES,
  PREVIEW_BURST_REVEAL,
  CAPSULE_APPEARANCE,
  buildPreviewRevealFrames,
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
assertEqual(
  CAPSULE_APPEARANCE.initialOpacity,
  1,
  'wake capsule should be visible on its first frame',
);
assertOk(
  CAPSULE_APPEARANCE.enterAnimMs <= 160,
  'wake capsule geometry should settle within 160 ms',
);

{
  const current = '这是已有的预览文字';
  const target = `${current}现在一次补回八个新字`;
  const frames = buildPreviewRevealFrames(current, target);
  assertOk(frames.length > 1, 'large pure append should be visually smoothed');
  assertOk(
    frames.length <= PREVIEW_BURST_REVEAL.maxFrames,
    'burst reveal should stay within the frame budget',
  );
  assertEqual(frames.at(-1), target, 'burst reveal must end at the exact provider text');
  for (const frame of frames) {
    assertOk(target.startsWith(frame), 'every reveal frame must be an exact target prefix');
  }
}

assertEqual(
  buildPreviewRevealFrames('开始路音', '开始录音').length,
  1,
  'non-prefix correction should apply immediately',
);
assertEqual(
  buildPreviewRevealFrames('开始录音', '开始录音。').length,
  1,
  'short append should apply immediately',
);

{
  const current = '你好';
  const target = `${current}A\u{1F642}B\u{1F680}C`;
  const frames = buildPreviewRevealFrames(current, target);
  assertEqual(frames.at(-1), target, 'Unicode reveal must preserve exact text');
  for (const frame of frames) {
    assertOk(!frame.includes('\uFFFD'), 'Unicode reveal must not split a code point');
  }
}

console.log('capsulePreviewRules: all assertions passed');
