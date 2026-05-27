import type { TFunction } from 'i18next';
import { probeEmbeddedAudioBleSubscription } from './ipc';
import { classifyEmbeddedBleProbeError } from './providerSetup';

export type EmbeddedBleProbeStatus = 'idle' | 'checking' | 'ok' | 'error';

const DEFAULT_PROBE_TIMEOUT_MS = 10_000;
const UI_TIMEOUT_GRACE_MS = 4_000;

class EmbeddedBleProbeUiTimeoutError extends Error {
  constructor(timeoutMs: number) {
    super(`embedded BLE check timed out after ${timeoutMs} ms`);
    this.name = 'EmbeddedBleProbeUiTimeoutError';
  }
}

export function runEmbeddedBleProbeWithTimeout(
  timeoutMs = DEFAULT_PROBE_TIMEOUT_MS,
): Promise<void> {
  return withTimeout(
    probeEmbeddedAudioBleSubscription(timeoutMs),
    timeoutMs + UI_TIMEOUT_GRACE_MS,
  );
}

export function embeddedBleProbeErrorMessage(error: unknown, t: TFunction): string {
  switch (classifyEmbeddedBleProbeError(error)) {
    case 'noDevice':
      return t('settings.recording.embeddedBleWakeGuidance', {
        defaultValue: '没找到 Listener 设备。若设备已深度睡眠，请按 KEY4/唤醒键后重试；仍失败时重新连接或导出诊断包。',
      });
    case 'accessDenied':
      return t('settings.recording.embeddedBleAccessDenied');
    case 'timeout':
      return t('settings.recording.embeddedBleWakeGuidance', {
        defaultValue: 'BLE 音频通路检查超时。若设备睡着，请按 KEY4/唤醒键后重试；仍失败时导出诊断包。',
      });
    case 'notify':
      return t('settings.recording.embeddedBleNotifyFailed');
    case 'generic':
    default:
      return t('settings.recording.embeddedBleWakeGuidance', {
        defaultValue: 'BLE 音频通路检查失败。请按 KEY4/唤醒键、重新连接设备，或导出诊断包。',
      });
  }
}

function withTimeout<T>(promise: Promise<T>, timeoutMs: number): Promise<T> {
  let timeoutId: number | undefined;
  const timeout = new Promise<T>((_, reject) => {
    timeoutId = window.setTimeout(
      () => reject(new EmbeddedBleProbeUiTimeoutError(timeoutMs)),
      timeoutMs,
    );
  });
  return Promise.race([promise, timeout]).finally(() => {
    if (timeoutId !== undefined) {
      window.clearTimeout(timeoutId);
    }
  });
}
