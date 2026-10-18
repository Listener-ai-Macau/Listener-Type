import assert from 'node:assert/strict';
import {
  getCapsuleDisplayMessage,
  isCapsuleQuotaErrorMessage,
  shouldRetainCapsulePreview,
} from './capsuleDisplayMessage.ts';

assert.equal(isCapsuleQuotaErrorMessage('当前识别资源额度不可用（错误码 45000292）；请核对资源 ID'), true);
assert.equal(isCapsuleQuotaErrorMessage('凭据被拒（403）'), false);
assert.equal(isCapsuleQuotaErrorMessage('今天讨论识别额度不足的问题'), false);
assert.equal(isCapsuleQuotaErrorMessage(undefined), false);

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
  getCapsuleDisplayMessage('done', '润色失败这几个字就是我说的原文'),
  '润色失败这几个字就是我说的原文',
  'real final text should not be hidden just because it contains similar words',
);

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
