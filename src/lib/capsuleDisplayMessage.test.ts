import assert from 'node:assert/strict';
import { getCapsuleDisplayMessage } from './capsuleDisplayMessage.ts';

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

console.log('capsuleDisplayMessage: all assertions passed');
