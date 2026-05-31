import type { DictationInputSource, DictationSession, EmbeddedAudioSessionStats } from './types';

export type ListenerDeviceHealthState =
  | 'healthy'
  | 'degraded'
  | 'error'
  | 'disconnected'
  | 'unsupported'
  | 'unknown';

export type ListenerDeviceHealthReason =
  | 'microphoneFallback'
  | 'unsupportedPlatform'
  | 'backgroundBleDisabled'
  | 'wakeRecovery'
  | 'needsWakeKey'
  | 'lowPowerIdleDisconnect'
  | 'staleGattCache'
  | 'missingPairing'
  | 'bluetoothUnavailable'
  | 'notifyRecovering'
  | 'historyUnavailable'
  | 'noBleEvidence'
  | 'staleBleEvidence'
  | 'completeAudio'
  | 'packetLoss'
  | 'emptyTranscript'
  | 'noAudio'
  | 'transportError'
  | 'bleCccdTimeout'
  | 'bleSubscriptionTimeout'
  | 'asrOrInsertError';

export interface ListenerDeviceHealthSnapshot {
  state: ListenerDeviceHealthState;
  reason: ListenerDeviceHealthReason;
  latestSession: DictationSession | null;
  stats: EmbeddedAudioSessionStats | null;
  ageMinutes: number | null;
}

export interface ListenerDeviceHealthInput {
  dictationInputSource: DictationInputSource | null | undefined;
  os: 'mac' | 'win' | 'linux' | 'unknown';
  history: DictationSession[];
  backgroundListenerDisabled?: boolean;
  backgroundListenerActive?: boolean;
  backgroundListenerReady?: boolean;
  backgroundListenerError?: string | null;
  wakeRecoveryStatus?: 'idle' | 'reconnecting' | 'ready' | 'needsWakeKey' | 'failed' | null;
  historyError?: boolean;
  now?: Date;
}

const STALE_BLE_EVIDENCE_MINUTES = 24 * 60;

export function summarizeListenerDeviceHealth({
  dictationInputSource,
  os,
  history,
  backgroundListenerDisabled = false,
  backgroundListenerActive = false,
  backgroundListenerReady = false,
  backgroundListenerError = null,
  wakeRecoveryStatus = null,
  historyError = false,
  now = new Date(),
}: ListenerDeviceHealthInput): ListenerDeviceHealthSnapshot {
  if (historyError) {
    return snapshot('unknown', 'historyUnavailable');
  }

  if ((dictationInputSource ?? 'microphone') !== 'embeddedBle') {
    return snapshot('disconnected', 'microphoneFallback');
  }

  if (os !== 'win') {
    return snapshot('unsupported', 'unsupportedPlatform');
  }

  if (backgroundListenerDisabled) {
    return snapshot('disconnected', 'backgroundBleDisabled');
  }

  if (wakeRecoveryStatus === 'reconnecting') {
    return snapshot('degraded', 'notifyRecovering');
  }
  if (wakeRecoveryStatus === 'needsWakeKey') {
    return snapshot('error', 'needsWakeKey');
  }
  if (wakeRecoveryStatus === 'failed') {
    return snapshot('error', 'wakeRecovery');
  }

  const listenerFailureReason = classifyBleSetupFailure(backgroundListenerError);
  if (listenerFailureReason) {
    return snapshot('error', listenerFailureReason);
  }

  if (backgroundListenerActive && backgroundListenerReady) {
    return snapshot('healthy', 'completeAudio');
  }

  const latestSession = history.find(session => session.embeddedAudioStats);
  if (!latestSession || !latestSession.embeddedAudioStats) {
    return snapshot('disconnected', 'noBleEvidence');
  }

  const stats = latestSession.embeddedAudioStats;
  const ageMinutes = sessionAgeMinutes(latestSession, now);
  if (ageMinutes != null && ageMinutes > STALE_BLE_EVIDENCE_MINUTES) {
    return snapshot('disconnected', 'staleBleEvidence', latestSession, stats, ageMinutes);
  }

  const setupFailureReason = classifyBleSetupFailure(latestSession.errorCode);
  if (setupFailureReason) {
    return snapshot('error', setupFailureReason, latestSession, stats, ageMinutes);
  }

  if (isCompleteAudioWithEmptyTranscript(latestSession, stats)) {
    return snapshot('error', 'emptyTranscript', latestSession, stats, ageMinutes);
  }

  if (stats.receivedPcmBytes <= 0 || stats.reconstructedPcmBytes <= 0) {
    return snapshot('error', 'noAudio', latestSession, stats, ageMinutes);
  }

  if (hasTransportError(stats)) {
    return snapshot('error', 'transportError', latestSession, stats, ageMinutes);
  }

  if (stats.missingPacketCount > 0) {
    return snapshot('degraded', 'packetLoss', latestSession, stats, ageMinutes);
  }

  if (latestSession.errorCode) {
    return snapshot('degraded', 'asrOrInsertError', latestSession, stats, ageMinutes);
  }

  return snapshot('healthy', 'completeAudio', latestSession, stats, ageMinutes);
}

export function listenerDeviceHealthTone(state: ListenerDeviceHealthState): 'ok' | 'err' | 'outline' {
  switch (state) {
    case 'healthy':
      return 'ok';
    case 'degraded':
    case 'error':
      return 'err';
    case 'disconnected':
    case 'unsupported':
    case 'unknown':
      return 'outline';
  }
}

function snapshot(
  state: ListenerDeviceHealthState,
  reason: ListenerDeviceHealthReason,
  latestSession: DictationSession | null = null,
  stats: EmbeddedAudioSessionStats | null = null,
  ageMinutes: number | null = null,
): ListenerDeviceHealthSnapshot {
  return { state, reason, latestSession, stats, ageMinutes };
}

function sessionAgeMinutes(session: DictationSession, now: Date): number | null {
  const createdAt = new Date(session.createdAt);
  if (!Number.isFinite(createdAt.getTime())) return null;
  return Math.max(0, Math.floor((now.getTime() - createdAt.getTime()) / 60000));
}

function classifyBleSetupFailure(errorCode: string | null): ListenerDeviceHealthReason | null {
  if (!errorCode) return null;
  const code = errorCode.toLowerCase();
  if (code.includes('no paired') || code.includes('not paired') || code.includes('missing pairing')) {
    return 'missingPairing';
  }
  if (code.includes('bluetooth service') || code.includes('radio') || code.includes('adapter') || code.includes('access denied')) {
    return 'bluetoothUnavailable';
  }
  if (code.includes('reason=546')
    || code.includes('reason: 546')
    || code.includes('reason 546')
    || code.includes('low-power idle')
    || code.includes('low power idle')
    || code.includes('idle disconnect')
    || code.includes('transport_not_ready')
    || code.includes('transport not ready')) {
    return 'lowPowerIdleDisconnect';
  }
  if (code.includes('stale') || code.includes('gatt cache') || code.includes('unknown gatt')) {
    return 'staleGattCache';
  }
  if (code.includes('deep sleep')
    || code.includes('asleep')
    || code.includes('sleeping')
    || code.includes('wake key')
    || code.includes('key4')) {
    return 'needsWakeKey';
  }
  if (code.includes('cccd') || code.includes('notify')) return 'bleCccdTimeout';
  if (code.includes('bletimeout') || code.includes('subscription') || code.includes('timeout')) {
    return 'bleSubscriptionTimeout';
  }
  return null;
}

function isCompleteAudioWithEmptyTranscript(
  session: DictationSession,
  stats: EmbeddedAudioSessionStats,
): boolean {
  return session.errorCode === 'emptyTranscript'
    && stats.terminalReceived
    && stats.missingPacketCount === 0
    && stats.reconstructedPcmBytes > 0;
}

function hasTransportError(stats: EmbeddedAudioSessionStats): boolean {
  if (!stats.terminalReceived) return true;
  if (stats.endReason == null) return false;
  if (stats.endReason === 'stop') return false;
  if (stats.endReason === 'cancel') return true;
  return 'error' in stats.endReason;
}
