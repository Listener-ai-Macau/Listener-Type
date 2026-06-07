import assert from 'node:assert/strict';
import {
  buildFirstRunPairingWizard,
  selectFirstRunPairingState,
  type FirstRunPairingWizardInput,
} from './firstRunPairingWizard.ts';
import { hasRawBleDiagnosticText } from './bleRecoveryUi.ts';
import type {
  EmbeddedBleFailureClassification,
  EmbeddedBleRepairResult,
  EmbeddedBleRuntimeStatus,
} from './types.ts';

const t = ((key: string) => {
  const copy: Record<string, string> = {
    'shell.blePairingPrompt.title': 'Pair your Listener device',
    'shell.blePairingPrompt.body': 'Find Listener, open Windows Bluetooth Add Device if needed, then check pairing.',
    'shell.blePairingPrompt.checkingTitle': 'Checking Listener device',
    'shell.blePairingPrompt.checkingBody': 'Checking pairing, connection, service discovery, and audio subscription.',
    'shell.blePairingPrompt.readyTitle': 'Listener is ready',
    'shell.blePairingPrompt.readyBody': 'Listener BLE is ready for the hardware voice key.',
    'shell.blePairingPrompt.needsPairingTitle': 'Pair in Windows Bluetooth',
    'shell.blePairingPrompt.needsPairingBody': 'Open Windows Bluetooth, choose Add device, pair Listener, then retry.',
    'shell.blePairingPrompt.pairedUnavailableTitle': 'Reconnect Listener',
    'shell.blePairingPrompt.pairedUnavailableBody': 'Listener is paired but unavailable. Retry or run recovery.',
    'shell.blePairingPrompt.needsRepairTitle': 'Repair Listener connection',
    'shell.blePairingPrompt.needsRepairBody': 'Run recovery, then retry Listener BLE.',
    'shell.blePairingPrompt.unsupportedBody': 'Listener BLE is available on Windows only.',
    'shell.blePairingPrompt.stage.discover.label': 'Find Listener',
    'shell.blePairingPrompt.stage.discover.description': 'Discover the target device',
    'shell.blePairingPrompt.stage.pair.label': 'Pair',
    'shell.blePairingPrompt.stage.pair.description': 'Windows Bluetooth Add Device',
    'shell.blePairingPrompt.stage.connect.label': 'Connect',
    'shell.blePairingPrompt.stage.connect.description': 'Reconnect to the paired device',
    'shell.blePairingPrompt.stage.service.label': 'Services',
    'shell.blePairingPrompt.stage.service.description': 'Read Listener services',
    'shell.blePairingPrompt.stage.subscribe.label': 'Subscribe',
    'shell.blePairingPrompt.stage.subscribe.description': 'Enable audio notifications',
  };
  return copy[key] ?? key;
}) as never;

const readyRuntime: EmbeddedBleRuntimeStatus = {
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
    lastReadyAt: '2026-06-07T00:00:00.000Z',
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
    userAction: 'recover',
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
    runtime: readyRuntime,
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

function input(overrides: Partial<FirstRunPairingWizardInput> = {}): FirstRunPairingWizardInput {
  return {
    supported: true,
    probeStatus: 'idle',
    probeMessage: null,
    runtime: null,
    lastRepairResult: null,
    ...overrides,
  };
}

assert.equal(selectFirstRunPairingState(input()), 'intro');
const intro = buildFirstRunPairingWizard(input(), t);
assert.equal(intro.primaryAction, 'startCheck');
assert.equal(intro.secondaryAction, 'openBluetooth');
assert.deepEqual(intro.stages.map(stage => stage.status), ['current', 'pending', 'pending', 'pending', 'pending']);
assert.match(intro.message, /Windows Bluetooth Add Device/);

const checkingSubscribe = buildFirstRunPairingWizard(input({
  probeStatus: 'checking',
  runtime: {
    ...readyRuntime,
    backgroundListenerReady: false,
    wakeRecovery: {
      ...readyRuntime.wakeRecovery,
      status: 'reconnecting',
      notifySubscriptionState: 'opening',
    },
  },
}), t);
assert.equal(checkingSubscribe.state, 'checking');
assert.deepEqual(checkingSubscribe.stages.map(stage => stage.status), ['done', 'done', 'done', 'done', 'current']);

assert.equal(selectFirstRunPairingState(input({ probeStatus: 'ok' })), 'ready');
assert.equal(selectFirstRunPairingState(input({ runtime: readyRuntime })), 'intro');
const repaired = buildFirstRunPairingWizard(input({
  lastRepairResult: repair('pairedButDisconnected', {
    recovered: true,
    userActionRequired: false,
    failure: null,
  }),
}), t);
assert.equal(repaired.state, 'ready');
assert.deepEqual(repaired.stages.map(stage => stage.status), ['done', 'done', 'done', 'done', 'done']);

const missingPairing = buildFirstRunPairingWizard(input({
  lastRepairResult: repair('missingPairing'),
}), t);
assert.equal(missingPairing.state, 'needsPairing');
assert.equal(missingPairing.primaryAction, 'openBluetooth');
assert.equal(missingPairing.secondaryAction, 'retry');
assert.equal(missingPairing.stages.find(stage => stage.id === 'pair')?.status, 'error');
assert.match(missingPairing.message, /Add device/);
assert.equal(hasRawBleDiagnosticText(missingPairing.message), false);

const unavailable = buildFirstRunPairingWizard(input({
  lastRepairResult: repair('pairedButDisconnected', {
    message: 'Listener is paired but unavailable. Press wake, then retry.',
  }),
}), t);
assert.equal(unavailable.state, 'pairedUnavailable');
assert.equal(unavailable.primaryAction, 'retry');
assert.equal(unavailable.secondaryAction, 'repair');
assert.equal(unavailable.stages.find(stage => stage.id === 'connect')?.status, 'error');
assert.match(unavailable.message, /retry/i);

const staleGatt = buildFirstRunPairingWizard(input({
  lastRepairResult: repair('staleGattService'),
}), t);
assert.equal(staleGatt.state, 'needsPairing');
assert.equal(staleGatt.stages.find(stage => stage.id === 'pair')?.status, 'error');
assert.equal(hasRawBleDiagnosticText(staleGatt.message), false);

const cccd = buildFirstRunPairingWizard(input({
  lastRepairResult: repair('cccdProtocolError'),
}), t);
assert.equal(cccd.state, 'needsPairing');
assert.equal(hasRawBleDiagnosticText(cccd.message), false);

const runtimeMissingPairing = buildFirstRunPairingWizard(input({
  runtime: {
    ...readyRuntime,
    backgroundListenerReady: false,
    backgroundListenerLastError: 'No paired BLE device for Listener',
  },
}), t);
assert.equal(runtimeMissingPairing.state, 'needsPairing');
assert.equal(runtimeMissingPairing.primaryAction, 'openBluetooth');

const rawProbe = buildFirstRunPairingWizard(input({
  probeStatus: 'error',
  probeMessage: 'WinRT DeviceInformation GATT failure on COM7 HRESULT 0x80070005',
}), t);
assert.equal(rawProbe.state, 'pairedUnavailable');
assert.equal(hasRawBleDiagnosticText(rawProbe.message), false);
