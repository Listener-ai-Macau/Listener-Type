import type { TFunction } from 'i18next';
import { hasRawBleDiagnosticText } from './bleRecoveryUi.ts';
import type { EmbeddedBleProbeStatus } from './embeddedBleProbe';
import type {
  EmbeddedBleFailureClassification,
  EmbeddedBleRepairResult,
  EmbeddedBleRuntimeStatus,
} from './types';

export type FirstRunPairingStageId = 'discover' | 'pair' | 'connect' | 'service' | 'subscribe';
export type FirstRunPairingStageStatus = 'pending' | 'current' | 'done' | 'error';
export type FirstRunPairingAction = 'startCheck' | 'openBluetooth' | 'retry' | 'repair' | 'done';
export type FirstRunPairingTone = 'outline' | 'blue' | 'ok' | 'err';

export interface FirstRunPairingStage {
  id: FirstRunPairingStageId;
  label: string;
  description: string;
  status: FirstRunPairingStageStatus;
}

export interface FirstRunPairingWizardInput {
  supported: boolean;
  probeStatus: EmbeddedBleProbeStatus;
  probeMessage?: string | null;
  runtime: EmbeddedBleRuntimeStatus | null;
  lastRepairResult: EmbeddedBleRepairResult | null;
}

export interface FirstRunPairingWizardModel {
  state: 'intro' | 'checking' | 'ready' | 'needsPairing' | 'pairedUnavailable' | 'needsRepair' | 'unsupported';
  title: string;
  message: string;
  tone: FirstRunPairingTone;
  stages: FirstRunPairingStage[];
  primaryAction: FirstRunPairingAction;
  secondaryAction: FirstRunPairingAction | null;
  showUseMicrophone: boolean;
}

type StageCopy = Record<FirstRunPairingStageId, { label: string; description: string }>;

const STAGE_ORDER: FirstRunPairingStageId[] = ['discover', 'pair', 'connect', 'service', 'subscribe'];
const BLUETOOTH_RECOVERY_FAILURES: EmbeddedBleFailureClassification['kind'][] = [
  'windowsBluetoothServiceResetNeeded',
  'accessDenied',
];
const PAIRING_FAILURES: EmbeddedBleFailureClassification['kind'][] = [
  'missingPairing',
  'staleGattService',
  'cccdProtocolError',
];
const PAIRED_UNAVAILABLE_FAILURES: EmbeddedBleFailureClassification['kind'][] = [
  'deviceMissing',
  'deviceAsleep',
  'lowPowerIdleDisconnect',
  'pairedButDisconnected',
  'backgroundListenerContention',
];

export function buildFirstRunPairingWizard(
  input: FirstRunPairingWizardInput,
  t: TFunction,
): FirstRunPairingWizardModel {
  const stageCopy = buildStageCopy(t);
  const state = selectFirstRunPairingState(input);
  const stageError = stageErrorForState(input, state);
  const activeStage = activeStageForState(input, state, stageError);
  const stages = buildStages(stageCopy, state, activeStage, stageError);
  const message = messageForState(input, state, t);

  switch (state) {
    case 'unsupported':
      return {
        state,
        title: t('shell.blePairingPrompt.title'),
        message,
        tone: 'outline',
        stages,
        primaryAction: 'done',
        secondaryAction: null,
        showUseMicrophone: true,
      };
    case 'checking':
      return {
        state,
        title: t('shell.blePairingPrompt.checkingTitle'),
        message,
        tone: 'blue',
        stages,
        primaryAction: 'retry',
        secondaryAction: null,
        showUseMicrophone: false,
      };
    case 'ready':
      return {
        state,
        title: t('shell.blePairingPrompt.readyTitle'),
        message,
        tone: 'ok',
        stages,
        primaryAction: 'done',
        secondaryAction: null,
        showUseMicrophone: false,
      };
    case 'needsPairing':
      return {
        state,
        title: t('shell.blePairingPrompt.needsPairingTitle'),
        message,
        tone: 'err',
        stages,
        primaryAction: 'openBluetooth',
        secondaryAction: 'retry',
        showUseMicrophone: true,
      };
    case 'pairedUnavailable':
      return {
        state,
        title: t('shell.blePairingPrompt.pairedUnavailableTitle'),
        message,
        tone: 'err',
        stages,
        primaryAction: 'retry',
        secondaryAction: 'repair',
        showUseMicrophone: true,
      };
    case 'needsRepair':
      return {
        state,
        title: t('shell.blePairingPrompt.needsRepairTitle'),
        message,
        tone: 'err',
        stages,
        primaryAction: 'repair',
        secondaryAction: 'retry',
        showUseMicrophone: true,
      };
    case 'intro':
    default:
      return {
        state: 'intro',
        title: t('shell.blePairingPrompt.title'),
        message,
        tone: 'outline',
        stages,
        primaryAction: 'startCheck',
        secondaryAction: 'openBluetooth',
        showUseMicrophone: true,
      };
  }
}

export function selectFirstRunPairingState({
  supported,
  probeStatus,
  runtime,
  lastRepairResult,
}: FirstRunPairingWizardInput): FirstRunPairingWizardModel['state'] {
  if (!supported) return 'unsupported';
  if (probeStatus === 'checking') return 'checking';
  if (probeStatus === 'ok' || lastRepairResult?.recovered) return 'ready';

  const repairFailure = lastRepairResult?.failure ?? null;
  if (repairFailure) {
    if (PAIRING_FAILURES.includes(repairFailure.kind)) return 'needsPairing';
    if (BLUETOOTH_RECOVERY_FAILURES.includes(repairFailure.kind)) return 'needsRepair';
    if (PAIRED_UNAVAILABLE_FAILURES.includes(repairFailure.kind)) return 'pairedUnavailable';
    if (repairFailure.kind === 'unsupportedPlatform') return 'unsupported';
    return 'needsRepair';
  }

  const runtimeFailure = classifyRuntimeFailure(runtime);
  if (runtimeFailure) return runtimeFailure;
  if (probeStatus === 'error') return runtimeReady(runtime) ? 'ready' : 'pairedUnavailable';
  return 'intro';
}

function buildStageCopy(t: TFunction): StageCopy {
  return {
    discover: {
      label: t('shell.blePairingPrompt.stage.discover.label'),
      description: t('shell.blePairingPrompt.stage.discover.description'),
    },
    pair: {
      label: t('shell.blePairingPrompt.stage.pair.label'),
      description: t('shell.blePairingPrompt.stage.pair.description'),
    },
    connect: {
      label: t('shell.blePairingPrompt.stage.connect.label'),
      description: t('shell.blePairingPrompt.stage.connect.description'),
    },
    service: {
      label: t('shell.blePairingPrompt.stage.service.label'),
      description: t('shell.blePairingPrompt.stage.service.description'),
    },
    subscribe: {
      label: t('shell.blePairingPrompt.stage.subscribe.label'),
      description: t('shell.blePairingPrompt.stage.subscribe.description'),
    },
  };
}

function buildStages(
  copy: StageCopy,
  state: FirstRunPairingWizardModel['state'],
  activeStage: FirstRunPairingStageId,
  stageError: FirstRunPairingStageId | null,
): FirstRunPairingStage[] {
  const activeIndex = STAGE_ORDER.indexOf(activeStage);
  const doneThrough = state === 'ready' ? STAGE_ORDER.length - 1 : Math.max(activeIndex - 1, -1);

  return STAGE_ORDER.map((id, index) => {
    let status: FirstRunPairingStageStatus = 'pending';
    if (index <= doneThrough) status = 'done';
    if (id === activeStage && state !== 'ready') status = 'current';
    if (id === stageError) status = 'error';
    return {
      id,
      label: copy[id].label,
      description: copy[id].description,
      status,
    };
  });
}

function activeStageForState(
  input: FirstRunPairingWizardInput,
  state: FirstRunPairingWizardModel['state'],
  stageError: FirstRunPairingStageId | null,
): FirstRunPairingStageId {
  if (stageError) return stageError;
  if (state === 'ready') return 'subscribe';
  if (state === 'needsPairing') return 'pair';
  if (state === 'pairedUnavailable') return 'connect';
  if (state === 'needsRepair') return failureStage(input.lastRepairResult?.failure ?? null);
  if (state === 'checking') return runtimeCheckingStage(input.runtime);
  return 'discover';
}

function stageErrorForState(
  input: FirstRunPairingWizardInput,
  state: FirstRunPairingWizardModel['state'],
): FirstRunPairingStageId | null {
  if (state === 'needsPairing') return 'pair';
  if (state === 'pairedUnavailable') return 'connect';
  if (state === 'needsRepair') return failureStage(input.lastRepairResult?.failure ?? null);
  return null;
}

function failureStage(failure: EmbeddedBleFailureClassification | null): FirstRunPairingStageId {
  switch (failure?.kind) {
    case 'missingPairing':
    case 'staleGattService':
      return 'pair';
    case 'cccdProtocolError':
      return 'subscribe';
    case 'missingDisFirmwareRevision':
      return 'service';
    case 'deviceMissing':
    case 'deviceAsleep':
    case 'lowPowerIdleDisconnect':
    case 'pairedButDisconnected':
    case 'backgroundListenerContention':
      return 'connect';
    case 'windowsBluetoothServiceResetNeeded':
    case 'accessDenied':
    case 'unknown':
    default:
      return 'service';
  }
}

function runtimeCheckingStage(runtime: EmbeddedBleRuntimeStatus | null): FirstRunPairingStageId {
  const notifyState = runtime?.wakeRecovery?.notifySubscriptionState;
  if (notifyState === 'subscribed') return 'subscribe';
  if (notifyState === 'opening' || notifyState === 'failed' || notifyState === 'cancelled') return 'subscribe';
  if (runtime?.wakeRecovery?.status === 'reconnecting') return 'connect';
  if (runtime?.backgroundListenerActive) return 'service';
  return 'discover';
}

function messageForState(
  input: FirstRunPairingWizardInput,
  state: FirstRunPairingWizardModel['state'],
  t: TFunction,
): string {
  switch (state) {
    case 'unsupported':
      return t('shell.blePairingPrompt.unsupportedBody');
    case 'checking':
      return t('shell.blePairingPrompt.checkingBody');
    case 'ready':
      return t('shell.blePairingPrompt.readyBody');
    case 'needsPairing':
      return t('shell.blePairingPrompt.needsPairingBody');
    case 'pairedUnavailable':
      return customerSafeWizardMessage(
        input.lastRepairResult?.message ?? input.probeMessage,
        t('shell.blePairingPrompt.pairedUnavailableBody'),
      );
    case 'needsRepair':
      return customerSafeWizardMessage(
        input.lastRepairResult?.message ?? input.probeMessage,
        t('shell.blePairingPrompt.needsRepairBody'),
      );
    case 'intro':
    default:
      return t('shell.blePairingPrompt.body');
  }
}

function customerSafeWizardMessage(value: string | null | undefined, fallback: string): string {
  const trimmed = value?.trim();
  if (!trimmed || hasRawBleDiagnosticText(trimmed)) return fallback;
  return trimmed;
}

function runtimeReady(runtime: EmbeddedBleRuntimeStatus | null): boolean {
  return runtime?.backgroundListenerReady === true
    && runtime.wakeRecovery?.status === 'ready'
    && runtime.wakeRecovery?.notifySubscriptionState === 'subscribed';
}

function classifyRuntimeFailure(runtime: EmbeddedBleRuntimeStatus | null): FirstRunPairingWizardModel['state'] | null {
  if (!runtime) return null;
  const combined = [
    runtime.backgroundListenerLastError,
    runtime.wakeRecovery?.recentDisconnectReason,
    runtime.wakeRecovery?.userGuidance,
  ]
    .filter((value): value is string => typeof value === 'string' && value.trim().length > 0)
    .join(' ')
    .toLowerCase();

  if (combined.includes('no paired') || combined.includes('not paired') || combined.includes('missing pairing')) {
    return 'needsPairing';
  }
  if (
    combined.includes('stale')
    || combined.includes('gatt cache')
    || combined.includes('unknown gatt')
    || combined.includes('cccd')
  ) {
    return 'needsPairing';
  }
  if (
    combined.includes('bluetooth service')
    || combined.includes('radio')
    || combined.includes('adapter')
    || combined.includes('access denied')
  ) {
    return 'needsRepair';
  }
  if (
    combined.includes('sleep')
    || combined.includes('wake')
    || combined.includes('not found')
    || combined.includes('disconnect')
    || combined.includes('transport_not_ready')
    || combined.includes('transport not ready')
  ) {
    return 'pairedUnavailable';
  }
  if (runtime.wakeRecovery?.status === 'needsWakeKey' || runtime.wakeRecovery?.status === 'failed') {
    return 'pairedUnavailable';
  }
  return null;
}
