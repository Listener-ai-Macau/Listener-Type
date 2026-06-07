import type { EmbeddedBleProbeErrorKind, ProviderConnectionErrorKind } from './providerSetup';

export type DemoModeSource = 'provider' | 'device';

export type DemoRecoveryAction =
  | 'configureProvider'
  | 'startDemo'
  | 'testAudio'
  | 'useMicrophone'
  | 'pairDevice'
  | 'retry';

export interface DemoRecoveryPath {
  source: DemoModeSource;
  reason: ProviderConnectionErrorKind | EmbeddedBleProbeErrorKind | 'providerMissingSetup';
  copyKey: string;
  primaryAction: DemoRecoveryAction;
  secondaryActions: DemoRecoveryAction[];
  offersDemo: boolean;
  offersTestAudio: boolean;
  writesProductionConfig: false;
  hidesProductionFailure: false;
}

export const DEMO_MODE_POLICY = {
  writesProductionConfig: false,
  persistsProductionConfig: false,
  hidesProductionFailure: false,
  preservesProductionFailureCopy: true,
} as const;

export interface DemoModeSession {
  source: DemoModeSource;
  policy: typeof DEMO_MODE_POLICY;
}

export function createDemoModeSession(source: DemoModeSource): DemoModeSession {
  return {
    source,
    policy: DEMO_MODE_POLICY,
  };
}

export function providerSetupDemoChoices(providersConfigured: boolean): DemoRecoveryPath | null {
  if (providersConfigured) return null;
  return {
    source: 'provider',
    reason: 'providerMissingSetup',
    copyKey: 'shell.demoMode.copy.missingSetup',
    primaryAction: 'startDemo',
    secondaryActions: ['configureProvider', 'testAudio'],
    offersDemo: true,
    offersTestAudio: true,
    writesProductionConfig: DEMO_MODE_POLICY.writesProductionConfig,
    hidesProductionFailure: DEMO_MODE_POLICY.hidesProductionFailure,
  };
}

export function providerDemoRecoveryForKind(kind: ProviderConnectionErrorKind): DemoRecoveryPath {
  if (kind === 'apiKeyRejected') {
    return providerPath(kind, 'shell.demoMode.copy.invalidKey', 'startDemo', ['configureProvider', 'testAudio']);
  }
  if (
    kind === 'network' ||
    kind === 'timeout' ||
    kind === 'providerUnavailable' ||
    kind === 'rateLimited'
  ) {
    return providerPath(kind, 'shell.demoMode.copy.offline', 'startDemo', ['testAudio', 'configureProvider']);
  }
  if (
    kind === 'apiKeyMissing' ||
    kind === 'endpointMissing' ||
    kind === 'endpointInvalid' ||
    kind === 'httpsRequired' ||
    kind === 'modelMissing' ||
    kind === 'modelsEmpty'
  ) {
    return providerPath(kind, 'shell.demoMode.copy.missingSetup', 'startDemo', ['configureProvider', 'testAudio']);
  }
  return providerPath(kind, 'shell.demoMode.copy.generic', 'testAudio', ['configureProvider', 'startDemo']);
}

export function embeddedBleDemoRecoveryForKind(kind: EmbeddedBleProbeErrorKind): DemoRecoveryPath {
  if (kind === 'noDevice') {
    return devicePath(kind, 'shell.demoMode.copy.noDevice', 'useMicrophone', ['pairDevice', 'testAudio']);
  }
  if (kind === 'accessDenied') {
    return devicePath(kind, 'shell.demoMode.copy.noDevice', 'useMicrophone', ['pairDevice', 'testAudio']);
  }
  return devicePath(kind, 'shell.demoMode.copy.deviceRecoverable', 'retry', ['useMicrophone', 'testAudio']);
}

function providerPath(
  reason: ProviderConnectionErrorKind,
  copyKey: string,
  primaryAction: DemoRecoveryAction,
  secondaryActions: DemoRecoveryAction[],
): DemoRecoveryPath {
  return {
    source: 'provider',
    reason,
    copyKey,
    primaryAction,
    secondaryActions,
    offersDemo: true,
    offersTestAudio: true,
    writesProductionConfig: DEMO_MODE_POLICY.writesProductionConfig,
    hidesProductionFailure: DEMO_MODE_POLICY.hidesProductionFailure,
  };
}

function devicePath(
  reason: EmbeddedBleProbeErrorKind,
  copyKey: string,
  primaryAction: DemoRecoveryAction,
  secondaryActions: DemoRecoveryAction[],
): DemoRecoveryPath {
  return {
    source: 'device',
    reason,
    copyKey,
    primaryAction,
    secondaryActions,
    offersDemo: true,
    offersTestAudio: true,
    writesProductionConfig: DEMO_MODE_POLICY.writesProductionConfig,
    hidesProductionFailure: DEMO_MODE_POLICY.hidesProductionFailure,
  };
}
