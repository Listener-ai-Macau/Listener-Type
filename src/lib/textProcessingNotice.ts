import type { StyleTextTrialReadiness } from './types';

/** An unavailable route is unknown, not evidence that the selected task is local. */
export function textProcessingNotice(native: boolean, offline: boolean, readiness: StyleTextTrialReadiness | null, canShorten: boolean): string {
  if (!native) return 'textWorkspace.previewProcessingNotice';
  if (offline) return 'offlineTranslation.localNotice';
  if (!readiness?.available) return 'textWorkspace.processingUnconfirmed';
  if (readiness.requiresTextService) return 'textWorkspace.cloudNotice';
  return canShorten ? 'textWorkspace.localWithShorterNotice' : 'textWorkspace.localNotice';
}
