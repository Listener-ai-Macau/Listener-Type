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
  backgroundListenerError?: string | null;
  historyError?: boolean;
  now?: Date;
}

const STALE_BLE_EVIDENCE_MINUTES = 24 * 60;

export function summarizeListenerDeviceHealth({
  dictationInputSource,
  os,
  history,
  backgroundListenerDisabled = false,
  backgroundListenerError = null,
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

  const listenerFailureReason = classifyBleSetupFailure(backgroundListenerError);
  if (listenerFailureReason) {
    return snapshot('error', listenerFailureReason);
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
