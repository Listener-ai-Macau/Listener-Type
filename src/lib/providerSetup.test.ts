import { readFileSync } from 'node:fs';
import {
  areProvidersConfigured,
  classifyEmbeddedBleProbeError,
  classifyProviderConnectionError,
  shouldShowProviderSetupPrompt,
} from './providerSetup.ts';

const providersSectionSource = readFileSync('src/pages/settings/ProvidersSection.tsx', 'utf8');
if (!providersSectionSource.includes(
  "const ASR_DEFAULT_RESOURCE_ID = 'volc.seedasr.sauc.duration';",
)) {
  throw new Error('Volcengine UI default must use the owner-provisioned Seed ASR 2.0 hourly quota');
}

function assertEqual(actual: boolean, expected: boolean, name: string) {
  if (actual !== expected) {
    throw new Error(`${name}: expected ${expected}, got ${actual}`);
  }
}

function assertStringEqual(actual: string, expected: string, name: string) {
  if (actual !== expected) {
    throw new Error(`${name}: expected ${expected}, got ${actual}`);
  }
}

assertEqual(
  areProvidersConfigured({
    activeAsrProvider: 'volcengine',
    activeLlmProvider: 'ark',
    asrConfigured: true,
    llmConfigured: true,
    volcengineConfigured: true,
    arkConfigured: true,
  }),
  true,
  'configured when ASR and LLM are both ready',
);

assertEqual(
  areProvidersConfigured({
    activeAsrProvider: 'volcengine',
    activeLlmProvider: 'ark',
    asrConfigured: false,
    llmConfigured: true,
    volcengineConfigured: false,
    arkConfigured: true,
  }),
  false,
  'not configured when ASR provider is missing',
);

assertEqual(
  areProvidersConfigured({
    activeAsrProvider: 'volcengine',
    activeLlmProvider: 'ark',
    asrConfigured: true,
    llmConfigured: false,
    volcengineConfigured: true,
    arkConfigured: false,
  }),
  false,
  'not configured when LLM provider is missing',
);

assertEqual(
  areProvidersConfigured({
    activeAsrProvider: 'whisper',
    activeLlmProvider: 'ark',
    asrConfigured: true,
    llmConfigured: true,
    volcengineConfigured: false,
    arkConfigured: true,
  }),
  true,
  'configured when active ASR is non-volcengine but already ready',
);

assertEqual(
  shouldShowProviderSetupPrompt(
    {
      activeAsrProvider: 'whisper',
      activeLlmProvider: 'ark',
      asrConfigured: false,
      llmConfigured: false,
      volcengineConfigured: false,
      arkConfigured: false,
    },
    null,
  ),
  true,
  'show first-run prompt when providers are missing and no prompt was seen',
);

assertEqual(
  shouldShowProviderSetupPrompt(
    {
      activeAsrProvider: 'whisper',
      activeLlmProvider: 'ark',
      asrConfigured: false,
      llmConfigured: false,
      volcengineConfigured: false,
      arkConfigured: false,
    },
    '1',
  ),
  false,
  'do not repeat first-run prompt after the user has deferred it in this session',
);

assertEqual(
  shouldShowProviderSetupPrompt(
    {
      activeAsrProvider: 'whisper',
      activeLlmProvider: 'ark',
      asrConfigured: true,
      llmConfigured: true,
      volcengineConfigured: false,
      arkConfigured: true,
    },
    null,
  ),
  false,
  'do not show prompt when providers are already configured',
);

assertStringEqual(
  classifyProviderConnectionError('providerHttpStatus:401'),
  'apiKeyRejected',
  'provider 401 becomes an API-key action, not a raw status',
);

assertStringEqual(
  classifyProviderConnectionError('providerHttpStatus:429'),
  'rateLimited',
  'provider 429 becomes a rate-limit action',
);

assertStringEqual(
  classifyProviderConnectionError('providerNetworkError'),
  'network',
  'provider network code becomes network action',
);

assertStringEqual(
  classifyProviderConnectionError('apiKeyMissing'),
  'apiKeyMissing',
  'backend missing API-key code becomes a concrete setup action',
);

assertStringEqual(
  classifyProviderConnectionError('endpointMissing'),
  'endpointMissing',
  'backend missing endpoint code becomes a concrete setup action',
);

assertStringEqual(
  classifyProviderConnectionError('providerInvalidModelList'),
  'responseInvalid',
  'invalid model-list JSON becomes a provider-response action',
);

assertStringEqual(
  classifyProviderConnectionError('llmModelMissing'),
  'modelMissing',
  'missing LLM model is a model action',
);

assertStringEqual(
  classifyEmbeddedBleProbeError('Embedded audio BLE service not found; ensure device is paired and online'),
  'noDevice',
  'BLE not-found probe error becomes no-device action',
);

assertStringEqual(
  classifyEmbeddedBleProbeError('BLE CCCD notify write returned status=AccessDenied'),
  'accessDenied',
  'BLE access denied probe error becomes permission action',
);

assertStringEqual(
  classifyEmbeddedBleProbeError('BLE embedded audio capture timed out after 30000 ms'),
  'timeout',
  'BLE capture timeout becomes timeout action',
);

assertStringEqual(
  classifyEmbeddedBleProbeError('Windows GATT disconnect reason=546 after low-power idle; transport_not_ready'),
  'timeout',
  'BLE idle disconnect becomes wake/retry timeout guidance',
);

assertStringEqual(
  classifyEmbeddedBleProbeError('BLE ValueChanged handler registration failed'),
  'notify',
  'BLE ValueChanged failures become notify action',
);

console.log('providerSetup: all assertions passed');
