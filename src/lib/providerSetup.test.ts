import {
  areProvidersConfigured,
  classifyEmbeddedBleProbeError,
  classifyProviderConnectionError,
  shouldShowProviderSetupPrompt,
} from './providerSetup.ts';

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
