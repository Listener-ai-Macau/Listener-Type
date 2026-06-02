import assert from 'node:assert/strict';
import {
  buildBleRecoveryUi,
  hasRawBleDiagnosticText,
  selectBleRecoveryUiState,
  type BleRecoveryUiInput,
} from './bleRecoveryUi.ts';
import type {
  EmbeddedBleFailureClassification,
  EmbeddedBleRepairResult,
  EmbeddedBleRuntimeStatus,
} from './types.ts';
import type { ListenerDeviceHealthSnapshot } from './deviceHealth.ts';

const t = ((key: string, fallback?: string) => fallback ?? key) as never;

const healthy: ListenerDeviceHealthSnapshot = {
  state: 'healthy',
  reason: 'completeAudio',
  latestSession: null,
  stats: null,
  ageMinutes: null,
};

const baseRuntime: EmbeddedBleRuntimeStatus = {
  backgroundListenerDisabledByEnv: false,
  backgroundListenerActive: true,
  backgroundListenerReady: true,
  backgroundListenerGeneration: 1,
  backgroundListenerLastError: null,
  wakeRecovery: {
    status: 'ready',
    userGuidance: 'ready',
    recentDisconnectReason: null,
    reconnectAttempts: 1,
    notifySubscriptionState: 'subscribed',
    firmwareWakePolicy: {
      policy: 'key4_only',
      wakeCapableKeys: 'KEY4',
      voiceKey: 'GPIO35',
      voiceKeyDeepSleepWake: false,
      readiness: 'ready',
      source: 'test',
    },
    lastAttemptAt: null,
    lastReadyAt: '2026-05-29T00:00:00.000Z',
  },
};

function failure(
  kind: EmbeddedBleFailureClassification['kind'],
  overrides: Partial<EmbeddedBleFailureClassification> = {},
): EmbeddedBleFailureClassification {
  return {
    kind,
    retryable: true,
    automaticRecovery: false,
    userAction: 'internal action',
    evidence: 'raw evidence',
    ...overrides,
  };
}

function repair(
  kind: EmbeddedBleFailureClassification['kind'],
  overrides: Partial<EmbeddedBleRepairResult> = {},
): EmbeddedBleRepairResult {
  return {
    recovered: false,
    userActionRequired: true,
    openBluetoothSettings: false,
    message: 'GATT CCCD HRESULT 0x80070490 on COM5',
    failure: failure(kind),
    runtime: baseRuntime,
    firmware: {
      connected: false,
      hardwareRevision: null,
      firmwareVersion: null,
      capabilities: [],
      batteryPercent: null,
      usbPowered: null,
      detail: null,
    },
    ...overrides,
  };
}

function input(overrides: Partial<BleRecoveryUiInput> = {}): BleRecoveryUiInput {
  return {
    supported: true,
    probeStatus: 'idle',
    probeMessage: null,
    runtime: baseRuntime,
    deviceHealth: healthy,
    lastRepairResult: null,
    ...overrides,
  };
}

assert.equal(selectBleRecoveryUiState(input({ supported: false })), 'unsupported');
assert.equal(selectBleRecoveryUiState(input({ probeStatus: 'checking' })), 'checking');
assert.equal(selectBleRecoveryUiState(input({ probeStatus: 'ok' })), 'ready');

const staleRuntime: EmbeddedBleRuntimeStatus = {
  ...baseRuntime,
  backgroundListenerLastError: 'Unknown GATT service from stale cache',
};
assert.equal(selectBleRecoveryUiState(input({ probeStatus: 'ok', runtime: staleRuntime })), 'ready');
assert.equal(
  selectBleRecoveryUiState(input({
    runtime: staleRuntime,
    lastRepairResult: repair('staleGattService', {
      recovered: true,
      userActionRequired: false,
      openBluetoothSettings: false,
      failure: null,
      runtime: baseRuntime,
    }),
  })),
  'ready',
);

assert.equal(selectBleRecoveryUiState(input({ lastRepairResult: repair('staleGattService') })), 'needsRePair');
assert.equal(selectBleRecoveryUiState(input({ lastRepairResult: repair('windowsBluetoothServiceResetNeeded') })), 'needsBluetooth');
assert.equal(selectBleRecoveryUiState(input({ lastRepairResult: repair('accessDenied') })), 'needsBluetooth');
assert.equal(selectBleRecoveryUiState(input({ lastRepairResult: repair('deviceMissing') })), 'needsWakeKey');
assert.equal(selectBleRecoveryUiState(input({ lastRepairResult: repair('deviceAsleep') })), 'needsWakeKey');
assert.equal(selectBleRecoveryUiState(input({ lastRepairResult: repair('missingPairing') })), 'needsRePair');
assert.equal(selectBleRecoveryUiState(input({ lastRepairResult: repair('lowPowerIdleDisconnect', { failure: failure('lowPowerIdleDisconnect', { automaticRecovery: true }) }) })), 'reconnecting');
assert.equal(selectBleRecoveryUiState(input({ lastRepairResult: repair('missingDisFirmwareRevision') })), 'diagnosticsAvailable');
assert.equal(selectBleRecoveryUiState(input({ lastRepairResult: repair('otaRebootWindow') })), 'otaReconnecting');
assert.equal(selectBleRecoveryUiState(input({ lastRepairResult: repair('cccdProtocolError', { failure: failure('cccdProtocolError', { automaticRecovery: true }) }) })), 'needsRePair');

assert.equal(
  selectBleRecoveryUiState(input({
    runtime: {
      ...baseRuntime,
      wakeRecovery: { ...baseRuntime.wakeRecovery, status: 'needsWakeKey' },
    },
    deviceHealth: { ...healthy, state: 'error', reason: 'needsWakeKey' },
  })),
  'needsWakeKey',
);

assert.equal(
  selectBleRecoveryUiState(input({
    runtime: {
      ...baseRuntime,
      wakeRecovery: { ...baseRuntime.wakeRecovery, status: 'reconnecting' },
    },
  })),
  'reconnecting',
);

assert.equal(
  selectBleRecoveryUiState(input({
    runtime: {
      ...baseRuntime,
      backgroundListenerLastError: 'BLE CCCD write timed out after 8000 ms',
      wakeRecovery: {
        ...baseRuntime.wakeRecovery,
        status: 'reconnecting',
        reconnectAttempts: 1,
        notifySubscriptionState: 'opening',
      },
    },
  })),
  'reconnecting',
);

assert.equal(
  selectBleRecoveryUiState(input({
    runtime: {
      ...baseRuntime,
      backgroundListenerLastError: 'BLE CCCD write timed out after 8000 ms',
      wakeRecovery: {
        ...baseRuntime.wakeRecovery,
        status: 'reconnecting',
        reconnectAttempts: 3,
        notifySubscriptionState: 'failed',
      },
    },
  })),
  'needsRePair',
);

assert.equal(
  selectBleRecoveryUiState(input({
    runtime: {
      ...baseRuntime,
      backgroundListenerLastError: 'GATT session still not active after 8000 ms current=Some(GattSessionStatus(0))',
      wakeRecovery: {
        ...baseRuntime.wakeRecovery,
        status: 'reconnecting',
        reconnectAttempts: 4,
        notifySubscriptionState: 'opening',
      },
    },
  })),
  'needsRePair',
);

assert.equal(
  selectBleRecoveryUiState(input({
    runtime: {
      ...baseRuntime,
      backgroundListenerLastError: 'Unknown GATT service from stale cache',
    },
  })),
  'needsRePair',
);

assert.equal(
  selectBleRecoveryUiState(input({
    runtime: {
      ...baseRuntime,
      backgroundListenerLastError: 'Windows GATT disconnect reason=546 after low-power idle; transport_not_ready',
    },
  })),
  'reconnecting',
);

assert.equal(
  selectBleRecoveryUiState(input({
    runtime: {
      ...baseRuntime,
      backgroundListenerLastError: 'Stale cached GATT path after reason=546 returned transport_not_ready',
    },
  })),
  'reconnecting',
);

assert.equal(
  selectBleRecoveryUiState(input({
    runtime: {
      ...baseRuntime,
      backgroundListenerLastError: 'No paired BLE device for Listener',
    },
  })),
  'needsRePair',
);

assert.equal(
  selectBleRecoveryUiState(input({
    runtime: {
      ...baseRuntime,
      backgroundListenerLastError: 'DIS firmware revision missing',
    },
  })),
  'diagnosticsAvailable',
);

assert.equal(
  selectBleRecoveryUiState(input({
    probeStatus: 'error',
    deviceHealth: { ...healthy, state: 'disconnected', reason: 'noBleEvidence' },
  })),
  'needsWakeKey',
);

assert.equal(hasRawBleDiagnosticText('GATT CCCD HRESULT 0x800706BA on COM5'), true);
assert.equal(hasRawBleDiagnosticText('Press the wake key and retry.'), false);
assert.equal(hasRawBleDiagnosticText('Restart Listener Type and come back here.'), false);

const missingUi = buildBleRecoveryUi(
  input({ lastRepairResult: repair('deviceMissing', { openBluetoothSettings: true }) }),
  t,
);
assert.equal(missingUi.state, 'needsWakeKey');
assert.equal(missingUi.showOpenBluetoothSettings, true);

const staleUi = buildBleRecoveryUi(
  input({
    probeStatus: 'error',
    probeMessage: 'GATT CCCD HRESULT 0x800706BA on COM5',
    lastRepairResult: repair('staleGattService'),
  }),
  t,
);
assert.equal(staleUi.state, 'needsRePair');
assert.equal(staleUi.emphasizeRePair, true);
assert.equal(staleUi.showOpenBluetoothSettings, true);
assert.equal(staleUi.showRepair, true);
assert.equal(staleUi.showExportDiagnostics, true);
assert.equal(hasRawBleDiagnosticText(staleUi.message), false);

const oneClickUi = buildBleRecoveryUi(
  input({
    probeStatus: 'error',
    lastRepairResult: repair('cccdProtocolError', {
      message: '旧的 Listener 蓝牙配对已清理。请在打开的 Windows 蓝牙设置里重新配对 Listener，Type 会自动恢复。',
    }),
  }),
  t,
);
assert.equal(oneClickUi.state, 'needsRePair');
assert.equal(oneClickUi.message, '旧的 Listener 蓝牙配对已清理。请在打开的 Windows 蓝牙设置里重新配对 Listener，Type 会自动恢复。');
assert.equal(hasRawBleDiagnosticText(oneClickUi.message), false);

const readyUi = buildBleRecoveryUi(input({ probeStatus: 'ok' }), t);
assert.equal(readyUi.state, 'ready');
assert.equal(readyUi.showDetails, false);
assert.equal(readyUi.showRepair, false);
assert.equal(readyUi.showExportDiagnostics, false);
