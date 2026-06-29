import type { CapsuleState } from './types';

const RECORDING_STARTUP_MESSAGES = new Set([
  '正在启动 Listener 录音...',
  '正在启动 Listener 录音',
  'Listener 录音已启动，正在接收音频...',
  'Listener 录音已启动，正在接收音频',
  '录音已启动',
  '音频已启动',
  'Recording started',
  'Audio started',
]);

const POLISH_FAILURE_DONE_MESSAGES = new Set([
  '润色失败，已插入原文',
  '潤色失敗，已插入原文',
  'Polish failed, inserted raw transcript',
]);

function normalizeCapsuleStatusMessage(message: string): string {
  return message.trim().replace(/\s+/g, ' ');
}

export function getCapsuleDisplayMessage(
  state: CapsuleState,
  message: string | null | undefined,
): string | undefined {
  if (message == null) return undefined;
  const normalized = normalizeCapsuleStatusMessage(message);
  if (!normalized) return undefined;
  if (state === 'recording' && RECORDING_STARTUP_MESSAGES.has(normalized)) {
    return undefined;
  }
  if (POLISH_FAILURE_DONE_MESSAGES.has(normalized)) {
    return undefined;
  }
  return message;
}
