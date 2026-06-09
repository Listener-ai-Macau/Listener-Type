import type { TFunction } from 'i18next';
import type { EmbeddedBleProbeStatus } from './embeddedBleProbe';
import type {
  EmbeddedBleFailureClassification,
  EmbeddedBleRepairResult,
  EmbeddedBleRuntimeStatus,
} from './types';
import type { ListenerDeviceHealthSnapshot } from './deviceHealth';

export type BleRecoveryUiState =
  | 'idle'
  | 'checking'
  | 'ready'
  | 'reconnecting'
  | 'needsWakeKey'
  | 'needsBluetooth'
  | 'needsRepair'
  | 'needsRePair'
  | 'otaReconnecting'
  | 'diagnosticsAvailable'
  | 'unsupported';

export interface BleRecoveryUiModel {
  state: BleRecoveryUiState;
  label: string;
  message: string;
  tone: 'outline' | 'ok' | 'blue' | 'err';
  showDetails: boolean;
  showOpenBluetoothSettings: boolean;
  showRepair: boolean;
  showUseMicrophone: boolean;
  showExportDiagnostics: boolean;
  emphasizeRePair: boolean;
}

export interface BleRecoveryUiInput {
  supported: boolean;
  probeStatus: EmbeddedBleProbeStatus;
  probeMessage?: string | null;
  runtime: EmbeddedBleRuntimeStatus | null;
  deviceHealth: ListenerDeviceHealthSnapshot;
  lastRepairResult: EmbeddedBleRepairResult | null;
}

type RecoveryCopyKey =
  | 'idle'
  | 'checking'
  | 'ready'
  | 'reconnecting'
  | 'needsWakeKey'
  | 'needsBluetooth'
  | 'needsRepair'
  | 'needsRePair'
  | 'otaReconnecting'
  | 'diagnosticsAvailable'
  | 'unsupported';

const LABEL_KEY_BY_COPY: Record<RecoveryCopyKey, string> = {
  idle: 'settings.recording.embeddedBleRecovery.idleLabel',
  checking: 'settings.recording.embeddedBleRecovery.checkingLabel',
  ready: 'settings.recording.embeddedBleRecovery.readyLabel',
  reconnecting: 'settings.recording.embeddedBleRecovery.reconnectingLabel',
  needsWakeKey: 'settings.recording.embeddedBleRecovery.needsWakeKeyLabel',
  needsBluetooth: 'settings.recording.embeddedBleRecovery.needsBluetoothLabel',
  needsRepair: 'settings.recording.embeddedBleRecovery.needsRepairLabel',
  needsRePair: 'settings.recording.embeddedBleRecovery.needsRePairLabel',
  otaReconnecting: 'settings.recording.embeddedBleRecovery.otaReconnectingLabel',
  diagnosticsAvailable: 'settings.recording.embeddedBleRecovery.diagnosticsAvailableLabel',
  unsupported: 'settings.recording.embeddedBleRecovery.unsupportedLabel',
};

const MESSAGE_KEY_BY_COPY: Record<RecoveryCopyKey, string> = {
  idle: 'settings.recording.embeddedBleRecovery.idleMessage',
  checking: 'settings.recording.embeddedBleRecovery.checkingMessage',
  ready: 'settings.recording.embeddedBleRecovery.readyMessage',
  reconnecting: 'settings.recording.embeddedBleRecovery.reconnectingMessage',
  needsWakeKey: 'settings.recording.embeddedBleRecovery.needsWakeKeyMessage',
  needsBluetooth: 'settings.recording.embeddedBleRecovery.needsBluetoothMessage',
  needsRepair: 'settings.recording.embeddedBleRecovery.needsRepairMessage',
  needsRePair: 'settings.recording.embeddedBleRecovery.needsRePairMessage',
  otaReconnecting: 'settings.recording.embeddedBleRecovery.otaReconnectingMessage',
  diagnosticsAvailable: 'settings.recording.embeddedBleRecovery.diagnosticsAvailableMessage',
  unsupported: 'settings.recording.embeddedBleRecovery.unsupportedMessage',
};

const DEFAULT_LABELS: Record<RecoveryCopyKey, string> = {
  idle: 'Not checked',
  checking: 'Checking',
  ready: 'Ready',
  reconnecting: 'Reconnecting',
  needsWakeKey: 'Wake device',
  needsBluetooth: 'Bluetooth',
  needsRepair: 'Needs repair',
  needsRePair: 'Re-pair',
  otaReconnecting: 'OTA reconnecting',
  diagnosticsAvailable: 'Diagnostics',
  unsupported: 'Unavailable',
};

const DEFAULT_MESSAGES: Record<RecoveryCopyKey, string> = {
  idle: 'Refresh Listener BLE when you want to confirm the device is ready.',
  checking: 'Checking Listener BLE and letting automatic recovery run.',
  ready: 'Listener BLE is ready for voice input.',
  reconnecting: 'Listener Type is reconnecting in the background. Wait a moment, then retry if it does not recover.',
  needsWakeKey: 'The device may be asleep. Press KEY4 or the wake key, then retry Listener BLE.',
  needsBluetooth: 'Turn on Windows Bluetooth, reconnect the Listener device, then retry.',
  needsRepair: 'Run One-click repair. If it still fails, export diagnostics.',
  needsRePair: 'Windows may have a stale Bluetooth pairing. Remove the Listener device in Windows Bluetooth, then pair it again.',
  otaReconnecting: 'The device is reconnecting after firmware update. Wait for it to return, then refresh status.',
  diagnosticsAvailable: 'Export diagnostics and contact support. No SDK, serial monitor, or COM port is required.',
  unsupported: 'Listener BLE audio is currently available on Windows only.',
};

export function buildBleRecoveryUi(input: BleRecoveryUiInput, t: TFunction): BleRecoveryUiModel {
  const state = selectBleRecoveryUiState(input);
  const copyKey = copyKeyForState(state);
  const tone = toneForState(state);
  const directFailureMessage = input.lastRepairResult && !input.lastRepairResult.recovered
    ? input.lastRepairResult.message
    : input.probeMessage;
  const message = input.probeStatus === 'error'
    ? customerSafeProbeMessage(directFailureMessage, state, t)
    : t(MESSAGE_KEY_BY_COPY[copyKey], DEFAULT_MESSAGES[copyKey]);
  const showDetails = state !== 'ready';

  return {
    state,
    label: t(LABEL_KEY_BY_COPY[copyKey], DEFAULT_LABELS[copyKey]),
    message,
    tone,
    showDetails,
    showOpenBluetoothSettings: state === 'needsBluetooth'
      || state === 'needsRePair'
      || input.lastRepairResult?.openBluetoothSettings === true,
    showRepair: input.supported
      && state !== 'ready'
      && state !== 'checking'
      && state !== 'unsupported'
      && state !== 'otaReconnecting',
    showUseMicrophone: input.supported && state !== 'ready' && state !== 'checking',
    showExportDiagnostics: input.supported && shouldShowDiagnostics(state),
    emphasizeRePair: state === 'needsRePair',
  };
}

export function selectBleRecoveryUiState({
  supported,
  probeStatus,
  runtime,
  deviceHealth,
  lastRepairResult,
}: BleRecoveryUiInput): BleRecoveryUiState {
  if (!supported) return 'unsupported';
  if (probeStatus === 'checking') return 'checking';
  if (probeStatus === 'ok' || lastRepairResult?.recovered) return 'ready';

  const repairFailure = lastRepairResult?.failure ?? null;
  if (lastRepairResult && !lastRepairResult.recovered && repairFailure) {
    return stateForFailure(repairFailure);
  }

  const runtimeFailure = classifyRuntimeFailure(runtime);
  if (runtimeFailure) return runtimeFailure;

  if (probeStatus === 'error') {
    return stateForDeviceHealth(deviceHealth) ?? 'needsRepair';
  }

  if (deviceHealth.state === 'healthy') return 'ready';
  return stateForDeviceHealth(deviceHealth) ?? 'idle';
}

export function hasRawBleDiagnosticText(value: string): boolean {
  const lower = value.toLowerCase();
  return lower.includes('gatt')
    || lower.includes('cccd')
    || /\bcom\d+\b/i.test(value)
    || lower.includes('hresult')
    || lower.includes('0x')
    || lower.includes('winrt')
    || lower.includes('deviceinformation');
}

function stateForFailure(failure: EmbeddedBleFailureClassification): BleRecoveryUiState {
  switch (failure.kind) {
    case 'staleGattService':
      return 'needsRePair';
    case 'windowsBluetoothServiceResetNeeded':
    case 'accessDenied':
      return 'needsBluetooth';
    case 'deviceMissing':
    case 'deviceAsleep':
      return 'needsWakeKey';
    case 'missingPairing':
      return 'needsRePair';
    case 'lowPowerIdleDisconnect':
      return failure.automaticRecovery ? 'reconnecting' : 'needsWakeKey';
    case 'missingDisFirmwareRevision':
      return 'diagnosticsAvailable';
    case 'otaRebootWindow':
      return 'otaReconnecting';
    case 'cccdProtocolError':
      return 'needsRePair';
    case 'pairedButDisconnected':
    case 'backgroundListenerContention':
      return failure.automaticRecovery ? 'reconnecting' : 'needsRepair';
    case 'unsupportedPlatform':
      return 'unsupported';
    case 'unknown':
    default:
      return failure.automaticRecovery ? 'reconnecting' : 'diagnosticsAvailable';
  }
}

function classifyRuntimeFailure(runtime: EmbeddedBleRuntimeStatus | null): BleRecoveryUiState | null {
  if (!runtime) return null;
  const combined = [
    runtime.backgroundListenerLastError,
    runtime.wakeRecovery?.recentDisconnectReason,
    runtime.wakeRecovery?.userGuidance,
  ]
    .filter((value): value is string => typeof value === 'string' && value.trim().length > 0)
    .join(' ')
    .toLowerCase();

  if (combined) {
    const highConfidenceState = stateForHighConfidenceRuntimeFailure(runtime, combined);
    if (highConfidenceState) return highConfidenceState;
  }

  const wakeStatus = runtime.wakeRecovery?.status;
  if (wakeStatus === 'reconnecting') return 'reconnecting';
  if (wakeStatus === 'needsWakeKey') return 'needsWakeKey';

  if (!combined) return null;
  if (combined.includes('ota') || combined.includes('reboot')) return 'otaReconnecting';
  if (combined.includes('firmware revision') || combined.includes('dis firmware')) {
    return 'diagnosticsAvailable';
  }
  if (combined.includes('no paired') || combined.includes('not paired') || combined.includes('missing pairing')) {
    return 'needsRePair';
  }
  if (combined.includes('bluetooth service') || combined.includes('radio') || combined.includes('adapter') || combined.includes('access denied')) {
    return 'needsBluetooth';
  }
  if (isStaleGattText(combined)) {
    return 'needsRePair';
  }
  if (runtimeAllowsLowPowerIdle(runtime) && runtimeTextSuggestsLowPowerIdle(combined)) {
    return 'reconnecting';
  }
  if (combined.includes('sleep') || combined.includes('wake') || combined.includes('not found')) {
    return 'needsWakeKey';
  }
  if (combined.includes('cccd') || combined.includes('notify') || combined.includes('cancelled') || combined.includes('background listener')) {
    return 'reconnecting';
  }
  if (wakeStatus === 'ready') return null;
  if (wakeStatus === 'failed') return 'needsRepair';
  return null;
}

function stateForHighConfidenceRuntimeFailure(
  runtime: EmbeddedBleRuntimeStatus,
  combined: string,
): BleRecoveryUiState | null {
  if (combined.includes('no paired') || combined.includes('not paired') || combined.includes('missing pairing')) {
    return 'needsRePair';
  }
  if (combined.includes('bluetooth service') || combined.includes('radio') || combined.includes('adapter') || combined.includes('access denied')) {
    return 'needsBluetooth';
  }
  if (combined.includes('firmware revision') || combined.includes('dis firmware')) {
    return 'diagnosticsAvailable';
  }

  if (isStaleGattText(combined)) {
    return 'needsRePair';
  }

  if (repeatedNotifySetupFailure(runtime, combined)) {
    return 'needsRePair';
  }

  return null;
}

function runtimeAllowsLowPowerIdle(runtime: EmbeddedBleRuntimeStatus): boolean {
  return runtime.wakeRecovery?.usbPowered === false;
}

function isStaleGattText(combined: string): boolean {
  return combined.includes('stale')
    || combined.includes('gatt cache')
    || combined.includes('unknown gatt');
}

function runtimeTextSuggestsLowPowerIdle(combined: string): boolean {
  const reason546 = combined.includes('reason=546')
    || combined.includes('reason: 546')
    || combined.includes('reason 546');
  const idleLabel = combined.includes('low-power idle')
    || combined.includes('low power idle')
    || combined.includes('idle disconnect');
  const transportNotReady = combined.includes('transport_not_ready')
    || combined.includes('transport not ready');
  const linkLoss = combined.includes('connection status changed')
    || combined.includes('gatt session status changed')
    || combined.includes('disconnected');

  return reason546 || idleLabel || (transportNotReady && linkLoss);
}

function repeatedNotifySetupFailure(runtime: EmbeddedBleRuntimeStatus, combined: string): boolean {
  const attempts = runtime.wakeRecovery?.reconnectAttempts ?? 0;
  if (attempts < 3) return false;
  if (runtimeAllowsLowPowerIdle(runtime) && runtimeTextSuggestsLowPowerIdle(combined)) return false;

  const notifyState = runtime.wakeRecovery?.notifySubscriptionState ?? 'unknown';
  const notifySetupStillUnavailable =
    notifyState === 'failed'
    || notifyState === 'opening'
    || notifyState === 'lost'
    || notifyState === 'unknown';
  if (!notifySetupStillUnavailable) return false;

  const notifyOrGattFailure = combined.includes('cccd')
    || combined.includes('notify write')
    || combined.includes('notify subscription')
    || combined.includes('gatt session still not active')
    || combined.includes('gattsessionstatus(0)')
    || combined.includes('bluetoothconnectionstatus(0)');
  const timeoutLike = combined.includes('timed out')
    || combined.includes('timeout')
    || combined.includes('not active')
    || combined.includes('disconnected');

  return notifyOrGattFailure && timeoutLike;
}

function stateForDeviceHealth(deviceHealth: ListenerDeviceHealthSnapshot): BleRecoveryUiState | null {
  switch (deviceHealth.reason) {
    case 'needsWakeKey':
    case 'noBleEvidence':
    case 'staleBleEvidence':
      return 'needsWakeKey';
    case 'lowPowerIdleDisconnect':
    case 'notifyRecovering':
      return 'reconnecting';
    case 'staleGattCache':
    case 'missingPairing':
      return 'needsRePair';
    case 'bluetoothUnavailable':
      return 'needsBluetooth';
    case 'wakeRecovery':
    case 'bleCccdTimeout':
    case 'bleSubscriptionTimeout':
    case 'transportError':
      return 'needsRepair';
    case 'backgroundBleDisabled':
      return 'needsBluetooth';
    case 'unsupportedPlatform':
      return 'unsupported';
    case 'historyUnavailable':
      return 'diagnosticsAvailable';
    case 'packetLoss':
    case 'emptyTranscript':
    case 'noAudio':
    case 'asrOrInsertError':
      return 'diagnosticsAvailable';
    case 'completeAudio':
      return deviceHealth.state === 'healthy' ? 'ready' : 'needsRepair';
    case 'microphoneFallback':
      return 'idle';
    default:
      return null;
  }
}

function copyKeyForState(state: BleRecoveryUiState): RecoveryCopyKey {
  return state;
}

function toneForState(state: BleRecoveryUiState): 'outline' | 'ok' | 'blue' | 'err' {
  switch (state) {
    case 'ready':
      return 'ok';
    case 'checking':
    case 'reconnecting':
    case 'otaReconnecting':
      return 'blue';
    case 'needsWakeKey':
    case 'needsBluetooth':
    case 'needsRepair':
    case 'needsRePair':
    case 'diagnosticsAvailable':
      return 'err';
    case 'idle':
    case 'unsupported':
      return 'outline';
  }
}

function shouldShowDiagnostics(state: BleRecoveryUiState): boolean {
  return state === 'needsRepair'
    || state === 'needsRePair'
    || state === 'needsBluetooth'
    || state === 'diagnosticsAvailable'
    || state === 'needsWakeKey';
}

function customerSafeProbeMessage(
  probeMessage: string | null | undefined,
  state: BleRecoveryUiState,
  t: TFunction,
): string {
  const trimmed = probeMessage?.trim();
  if (trimmed && !hasRawBleDiagnosticText(trimmed)) {
    return trimmed;
  }
  const copyKey = copyKeyForState(state);
  return t(MESSAGE_KEY_BY_COPY[copyKey], DEFAULT_MESSAGES[copyKey]);
}
