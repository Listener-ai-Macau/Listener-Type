import type { CredentialsStatus } from './types';

export const PROVIDER_SETUP_PROMPT_DEFERRED_KEY = 'ol.providerSetupPromptDeferredThisSession';

export type ProviderConnectionErrorKind =
  | 'apiKeyRejected'
  | 'rateLimited'
  | 'providerUnavailable'
  | 'network'
  | 'timeout'
  | 'apiKeyMissing'
  | 'endpointMissing'
  | 'endpointInvalid'
  | 'httpsRequired'
  | 'modelMissing'
  | 'modelsEmpty'
  | 'responseInvalid'
  | 'proxy'
  | 'generic';

export type EmbeddedBleProbeErrorKind =
  | 'noDevice'
  | 'accessDenied'
  | 'timeout'
  | 'notify'
  | 'generic';

export function areProvidersConfigured(credentials: CredentialsStatus): boolean {
  const asrConfigured = credentials.asrConfigured ?? credentials.volcengineConfigured;
  const llmConfigured = credentials.llmConfigured ?? credentials.arkConfigured;
  return asrConfigured && llmConfigured;
}

export function shouldShowProviderSetupPrompt(
  credentials: CredentialsStatus,
  promptDeferredValue: string | null,
): boolean {
  return !areProvidersConfigured(credentials) && promptDeferredValue !== '1';
}

function errorText(error: unknown): string {
  if (typeof error === 'string') return error;
  if (error instanceof Error) return error.message;
  return String(error);
}

export function classifyProviderConnectionError(error: unknown): ProviderConnectionErrorKind {
  const message = errorText(error);
  const lower = message.toLowerCase();

  if (message === 'llmModelMissing' || message === 'asrModelMissing') return 'modelMissing';
  if (message === 'modelsEmpty') return 'modelsEmpty';
  if (message === 'endpointMustUseHttps') return 'httpsRequired';
  if (message === 'endpointInvalid') return 'endpointInvalid';
  if (message === 'providerRequestTimeout' || lower.includes('timeout') || message.includes('超时')) {
    return 'timeout';
  }
  if (message === 'providerNetworkError' || lower.startsWith('network error') || message.startsWith('网络错误')) {
    return 'network';
  }
  if (message === 'providerReadResponseFailed' || message === 'providerClientInitFailed') {
    return 'network';
  }
  if (message === 'providerInvalidModelList' || message === 'providerResponseTooLarge' || message === 'asrInvalidJson' || message === 'asrMissingTextField') {
    return 'responseInvalid';
  }
  if (message === 'proxyUrlMissing' || message === 'proxyUrlInvalid' || message === 'proxyModeInvalid') {
    return 'proxy';
  }
  if (message.includes('API Key') || message.includes('api key') || message === 'apiKeyMissing') {
    return 'apiKeyMissing';
  }
  if (message.includes('Endpoint') || message === 'endpointMissing') {
    return 'endpointMissing';
  }
  if (message.startsWith('providerHttpStatus:')) {
    const status = Number.parseInt(message.split(':')[1] ?? '', 10);
    if (status === 401 || status === 403) return 'apiKeyRejected';
    if (status === 429) return 'rateLimited';
    if (status >= 500 && status <= 599) return 'providerUnavailable';
    return 'generic';
  }
  return 'generic';
}

export function classifyEmbeddedBleProbeError(error: unknown): EmbeddedBleProbeErrorKind {
  const message = errorText(error);
  const lower = message.toLowerCase();

  if (
    lower.includes('not found') ||
    lower.includes('no subscribable') ||
    lower.includes('service discovery returned status=unreachable') ||
    message.includes('未找到')
  ) {
    return 'noDevice';
  }
  if (lower.includes('access denied') || lower.includes('denied') || message.includes('拒绝')) {
    return 'accessDenied';
  }
  if (lower.includes('timed out') || lower.includes('timeout') || message.includes('超时')) {
    return 'timeout';
  }
  if (lower.includes('notify') || lower.includes('cccd') || lower.includes('valuechanged')) {
    return 'notify';
  }
  return 'generic';
}
