import type { VoiceprintStatus } from './types';

export function voiceprintFailureUi(status: Pick<VoiceprintStatus, 'failureStage' | 'canRetrySave'> | null) {
  const stage = status?.failureStage;
  if (stage === 'storage') {
    return { prefix: 'voiceprintSaveError', retry: status?.canRetrySave ? 'save' : 'none', sampleError: false } as const;
  }
  if (stage === 'capture' || stage === 'phrase') {
    return { prefix: 'voiceprintWizardError', retry: 'record', sampleError: true } as const;
  }
  if (stage === 'device') {
    return { prefix: 'voiceprintDeviceError', retry: 'record', sampleError: false } as const;
  }
  return { prefix: 'voiceprintEnrollmentError', retry: 'record', sampleError: false } as const;
}
