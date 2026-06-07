import {
  DEMO_MODE_POLICY,
  createDemoModeSession,
  embeddedBleDemoRecoveryForKind,
  providerDemoRecoveryForKind,
  providerSetupDemoChoices,
} from './demoMode.ts';
import {
  classifyEmbeddedBleProbeError,
  classifyProviderConnectionError,
} from './providerSetup.ts';

function assertEqual<T>(actual: T, expected: T, name: string) {
  if (actual !== expected) {
    throw new Error(`${name}: expected ${String(expected)}, got ${String(actual)}`);
  }
}

function assertIncludes<T>(actual: T[], expected: T, name: string) {
  if (!actual.includes(expected)) {
    throw new Error(`${name}: expected ${JSON.stringify(actual)} to include ${String(expected)}`);
  }
}

function assertPresent<T>(actual: T | null | undefined, name: string): T {
  if (actual == null) {
    throw new Error(`${name}: expected value to be present`);
  }
  return actual;
}

assertEqual(DEMO_MODE_POLICY.writesProductionConfig, false, 'demo mode never writes production config');
assertEqual(DEMO_MODE_POLICY.persistsProductionConfig, false, 'demo mode never persists provider or device config');
assertEqual(DEMO_MODE_POLICY.hidesProductionFailure, false, 'demo mode keeps real production failures visible');
assertEqual(DEMO_MODE_POLICY.preservesProductionFailureCopy, true, 'demo mode preserves recovery copy');

const providerSession = createDemoModeSession('provider');
assertEqual(providerSession.source, 'provider', 'provider demo session source');
assertEqual(providerSession.policy, DEMO_MODE_POLICY, 'provider demo session uses shared policy');

const noKeyChoices = assertPresent(
  providerSetupDemoChoices(false),
  'missing provider setup returns demo choices',
);
assertEqual(noKeyChoices.reason, 'providerMissingSetup', 'no-key setup reason');
assertEqual(noKeyChoices.offersDemo, true, 'no-key setup offers demo mode');
assertEqual(noKeyChoices.offersTestAudio, true, 'no-key setup offers test audio');
assertEqual(noKeyChoices.primaryAction, 'startDemo', 'no-key setup has a clear next action');
assertIncludes(noKeyChoices.secondaryActions, 'configureProvider', 'no-key setup can recover with provider settings');
assertIncludes(noKeyChoices.secondaryActions, 'testAudio', 'no-key setup can test audio without credentials');

assertEqual(
  providerSetupDemoChoices(true),
  null,
  'configured providers do not show demo recovery choices',
);

const invalidKey = providerDemoRecoveryForKind(classifyProviderConnectionError('providerHttpStatus:401'));
assertEqual(invalidKey.reason, 'apiKeyRejected', 'invalid key classified for demo recovery');
assertEqual(invalidKey.copyKey, 'shell.demoMode.copy.invalidKey', 'invalid key has specific recovery copy');
assertEqual(invalidKey.primaryAction, 'startDemo', 'invalid key can continue into demo mode');
assertIncludes(invalidKey.secondaryActions, 'configureProvider', 'invalid key keeps API-key repair visible');
assertEqual(invalidKey.writesProductionConfig, false, 'invalid key demo does not write config');
assertEqual(invalidKey.hidesProductionFailure, false, 'invalid key demo does not hide production failure');

const offline = providerDemoRecoveryForKind(classifyProviderConnectionError('providerNetworkError'));
assertEqual(offline.reason, 'network', 'offline/network classified for demo recovery');
assertEqual(offline.copyKey, 'shell.demoMode.copy.offline', 'offline has specific recovery copy');
assertEqual(offline.offersDemo, true, 'offline offers demo mode');
assertEqual(offline.offersTestAudio, true, 'offline offers test audio');
assertIncludes(offline.secondaryActions, 'configureProvider', 'offline still offers provider repair');

const missingKey = providerDemoRecoveryForKind(classifyProviderConnectionError('apiKeyMissing'));
assertEqual(missingKey.reason, 'apiKeyMissing', 'api-key missing classified for demo recovery');
assertEqual(missingKey.copyKey, 'shell.demoMode.copy.missingSetup', 'missing key has setup recovery copy');
assertEqual(missingKey.primaryAction, 'startDemo', 'missing key can continue into demo mode');

const noDevice = embeddedBleDemoRecoveryForKind(
  classifyEmbeddedBleProbeError('Embedded audio BLE service not found; ensure device is paired and online'),
);
assertEqual(noDevice.reason, 'noDevice', 'no-device classified for demo recovery');
assertEqual(noDevice.copyKey, 'shell.demoMode.copy.noDevice', 'no-device has specific recovery copy');
assertEqual(noDevice.primaryAction, 'useMicrophone', 'no-device uses microphone as the clear next action');
assertIncludes(noDevice.secondaryActions, 'pairDevice', 'no-device keeps pairing repair visible');
assertIncludes(noDevice.secondaryActions, 'testAudio', 'no-device offers test-audio path');
assertEqual(noDevice.writesProductionConfig, false, 'no-device demo does not write config');
assertEqual(noDevice.hidesProductionFailure, false, 'no-device demo does not hide production failure');

const deviceRecoverable = embeddedBleDemoRecoveryForKind(
  classifyEmbeddedBleProbeError('BLE embedded audio capture timed out after 30000 ms'),
);
assertEqual(deviceRecoverable.reason, 'timeout', 'recoverable device error classified for demo recovery');
assertEqual(deviceRecoverable.primaryAction, 'retry', 'recoverable device error keeps retry as next action');
assertIncludes(deviceRecoverable.secondaryActions, 'useMicrophone', 'recoverable device error can fall back to microphone');

console.log('demoMode: all assertions passed');
