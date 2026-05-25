import assert from 'node:assert/strict';
import {
  summarizeListenerDeviceHealth,
  type ListenerDeviceHealthReason,
  type ListenerDeviceHealthState,
} from './deviceHealth.ts';
import type { DictationSession, EmbeddedAudioSessionStats } from './types.ts';

const now = new Date('2026-05-25T12:00:00.000Z');

function stats(overrides: Partial<EmbeddedAudioSessionStats> = {}): EmbeddedAudioSessionStats {
  return {
    sessionId: 7,
    explicitStartReceived: true,
    startInferredFromAudio: false,
    terminalReceived: true,
    endReason: 'stop',
    expectedPacketCount: 4,
    receivedPacketCount: 4,
    missingPacketCount: 0,
    missingPacketIndices: [],
    receivedPcmBytes: 3200,
    reconstructedPcmBytes: 3200,
    silenceFilledBytes: 0,
    duplicatePacketCount: 0,
    replacedPacketCount: 0,
    ignoredForeignPacketCount: 0,
    durationSeconds: 1,
    ...overrides,
  };
}

function session(overrides: Partial<DictationSession> = {}): DictationSession {
  return {
    id: 'session-1',
    createdAt: '2026-05-25T11:55:00.000Z',
    rawTranscript: 'test',
    finalText: 'test',
    mode: 'structured',
    appBundleId: null,
    appName: null,
    insertStatus: 'inserted',
    errorCode: null,
    durationMs: 1000,
    dictionaryEntryCount: 0,
    hasAudioRecording: false,
    embeddedAudioStats: stats(),
    ...overrides,
  };
}

function expectHealth(
  input: Parameters<typeof summarizeListenerDeviceHealth>[0],
  state: ListenerDeviceHealthState,
  reason: ListenerDeviceHealthReason,
) {
  const result = summarizeListenerDeviceHealth({ now, ...input });
  assert.equal(result.state, state);
  assert.equal(result.reason, reason);
}

expectHealth(
  { dictationInputSource: 'microphone', os: 'win', history: [session()] },
  'disconnected',
  'microphoneFallback',
);

expectHealth(
  { dictationInputSource: 'embeddedBle', os: 'linux', history: [session()] },
  'unsupported',
  'unsupportedPlatform',
);

expectHealth(
  { dictationInputSource: 'embeddedBle', os: 'win', history: [] },
  'disconnected',
  'noBleEvidence',
);

expectHealth(
  { dictationInputSource: 'embeddedBle', os: 'win', history: [session()], backgroundListenerDisabled: true },
  'disconnected',
  'backgroundBleDisabled',
);

expectHealth(
  { dictationInputSource: 'embeddedBle', os: 'win', history: [session()] },
  'healthy',
  'completeAudio',
);

expectHealth(
  {
    dictationInputSource: 'embeddedBle',
    os: 'win',
    history: [session({ embeddedAudioStats: stats({ missingPacketCount: 1, missingPacketIndices: [3] }) })],
  },
  'degraded',
  'packetLoss',
);

expectHealth(
  {
    dictationInputSource: 'embeddedBle',
    os: 'win',
    history: [session({ finalText: '', errorCode: 'emptyTranscript' })],
  },
  'error',
  'emptyTranscript',
);

expectHealth(
  {
    dictationInputSource: 'embeddedBle',
    os: 'win',
    history: [session({ errorCode: 'bleNotifyCccdTimeout' })],
  },
  'error',
  'bleCccdTimeout',
);

expectHealth(
  {
    dictationInputSource: 'embeddedBle',
    os: 'win',
    history: [session({ embeddedAudioStats: stats({ terminalReceived: false }) })],
  },
  'error',
  'transportError',
);
