import assert from 'node:assert/strict';
import {
  getCapsuleDisplayMessage,
  isCapsuleQuotaErrorMessage,
  shouldRetainCapsulePreview,
  shouldShowCapsuleQuotaError,
  shouldShowCapsuleNetworkStatus,
} from './capsuleDisplayMessage.ts';

assert.equal(isCapsuleQuotaErrorMessage('当前识别资源额度不可用（错误码 45000292）；请核对资源 ID'), true);
assert.equal(isCapsuleQuotaErrorMessage('凭据被拒（403）'), false);
assert.equal(isCapsuleQuotaErrorMessage('今天讨论识别额度不足的问题'), false);
assert.equal(isCapsuleQuotaErrorMessage(undefined), false);

for (const state of ['recording', 'transcribing', 'polishing'] as const) {
  assert.equal(shouldShowCapsuleNetworkStatus(state), true);
  assert.equal(shouldShowCapsuleQuotaError(state, true), true,
    'a real quota failure takes display priority over a partial preview or finalization spinner');
  assert.equal(shouldShowCapsuleQuotaError(state, false), false,
    'a healthy provider keeps the existing streaming preview');
}
for (const state of ['idle', 'done', 'cancelled', 'error', 'reconnecting'] as const) {
  assert.equal(shouldShowCapsuleQuotaError(state, true), false,
    'a retained provider warning cannot replace completion, cancellation or another specific error');
}

for (const state of ['idle', 'done', 'cancelled', 'error'] as const) {
  assert.equal(shouldShowCapsuleNetworkStatus(state), false,
    'a retained network advisory must not reappear above completion or another terminal state');
}
assert.equal(shouldShowCapsuleNetworkStatus('reconnecting'), true);

assert.equal(
  getCapsuleDisplayMessage('recording', 'Listener 录音已启动，正在接收音频...'),
  undefined,
  'recording startup confirmation should use waveform instead of text',
);

assert.equal(
  getCapsuleDisplayMessage('recording', '正在启动 Listener 录音...'),
  undefined,
  'recording startup pending message should use waveform instead of text',
);

assert.equal(
  getCapsuleDisplayMessage('recording', '正在接管当前录音...'),
  undefined,
  'legacy hidden-VA promote copy must not surface on ordinary press',
);

assert.equal(
  getCapsuleDisplayMessage('recording', '今天下午三点开会'),
  '今天下午三点开会',
  'real recording preview text should remain visible',
);

assert.equal(
  getCapsuleDisplayMessage('done', '润色失败，已插入原文'),
  undefined,
  'polish failure fallback status should not be shown in the capsule',
);

assert.equal(
  getCapsuleDisplayMessage('recording', '润色失败这几个字就是我说的原文'),
  '润色失败这几个字就是我说的原文',
  'real recording text should not be hidden just because it contains similar words',
);

for (const message of [
  '完整文字已复制，请核对上屏内容',
  '上屏内容可能不完整，完整文字在剪贴板；请核对后替换',
  '上屏内容可能不完整，完整文字已保存在历史记录',
  '输入事件已发送，但 Listener 未收到目标确认；请检查光标位置',
  '剪贴板未能更新，完整文字可在历史记录复制',
  '未找到输入框，正文已保存在历史记录',
  '任意未来的完成诊断',
]) {
  assert.equal(getCapsuleDisplayMessage('done', message), undefined,
    'every completion uses the completion mark, including replayed and combined notices');
  assert.equal(getCapsuleDisplayMessage('error', message), message,
    'an actual error state must still show its error message');
}

assert.equal(
  getCapsuleDisplayMessage('reconnecting', '正在等待 Listener 音频...'),
  '正在等待 Listener 音频...',
  'actionable waiting messages should remain visible',
);

assert.equal(
  shouldRetainCapsulePreview({
    state: 'recording',
    sessionId: 's1',
    messageSessionId: 's1',
    currentMessage: '今天下午',
  }),
  true,
  'same-session PCM ticks must keep the live recording preview',
);

assert.equal(
  shouldRetainCapsulePreview({
    state: 'recording',
    sessionId: null,
    messageSessionId: 's1',
    currentMessage: '今天下午',
  }),
  true,
  'level ticks that omit session id must not wipe the current preview',
);

assert.equal(
  shouldRetainCapsulePreview({
    state: 'recording',
    sessionId: 's2',
    messageSessionId: 's1',
    currentMessage: '今天下午',
  }),
  false,
  'a different session may replace the previous preview',
);

assert.equal(
  shouldRetainCapsulePreview({
    state: 'recording',
    sessionId: 's1',
    messageSessionId: 's1',
    currentMessage: undefined,
  }),
  false,
  'there is nothing to retain before the first body preview',
);

assert.equal(
  shouldRetainCapsulePreview({
    state: 'idle',
    sessionId: 's1',
    messageSessionId: 's1',
    currentMessage: '今天下午',
  }),
  false,
  'idle must still be allowed to clear the capsule',
);

for (const messageSessionId of [null, 'previous-session']) {
  assert.equal(
    shouldRetainCapsulePreview({
      state: 'recording',
      sessionId: 'new-session',
      messageSessionId,
      currentMessage: 'Previous completion or error',
    }),
    false,
    'a new recording must clear both dismissed and session-owned old messages',
  );
}

assert.equal(
  shouldRetainCapsulePreview({
    state: 'recording',
    sessionId: 's1',
    messageSessionId: 's1',
    currentMessage: '旁人对白',
    incomingMessage: '',
  }),
  false,
  'a confirmed empty owner partition must clear provisional foreign-only text',
);

for (const incomingMessage of [undefined, null, '正在启动 Listener 录音...']) {
  assert.equal(
    shouldRetainCapsulePreview({
      state: 'recording',
      sessionId: 's1',
      messageSessionId: 's1',
      currentMessage: '本人正文继续保留',
      incomingMessage,
    }),
    true,
    'level ticks and suppressed startup messages must still retain owner text',
  );
}

console.log('capsuleDisplayMessage: all assertions passed');
