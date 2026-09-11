import type { CapsuleState } from './types';

const RECORDING_STARTUP_MESSAGES = new Set([
  '正在启动 Listener 录音...',
  '正在启动 Listener 录音',
  // Legacy promote-path copy (backend no longer emits; keep suppressed if stale).
  '正在接管当前录音...',
  '正在接管当前录音',
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

const PREVIEW_RETAIN_STATES: ReadonlySet<CapsuleState> = new Set([
  'recording',
  'transcribing',
  'polishing',
]);

/**
 * Level-only ticks and filtered startup copy must not wipe a live body
 * preview. Session 505f79e4 streamed 2→27 chars from the backend, then a
 * PCM tick with message=None arrived 7ms later and the capsule looked empty
 * until the 27-char dump.
 */
export function shouldRetainCapsulePreview(options: {
  state: CapsuleState;
  sessionId: string | null;
  messageSessionId: string | null;
  currentMessage: string | undefined;
}): boolean {
  if (!options.currentMessage) return false;
  if (!PREVIEW_RETAIN_STATES.has(options.state)) return false;
  if (
    options.sessionId
    && options.messageSessionId
    && options.sessionId !== options.messageSessionId
  ) {
    return false;
  }
  return true;
}
