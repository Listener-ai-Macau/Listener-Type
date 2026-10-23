import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { CREDENTIALS_STATUS_CHANGED_EVENT } from '../lib/credentialsStatusEvents';
import { getStyleTextTrialReadiness } from '../lib/ipc';
import type { StyleTextTrialReadiness } from '../lib/types';
import { Btn } from '../pages/_atoms';

/** Uses the same saved-style policy as the trial, including customized Raw packs. */
export function StyleTrialAvailability({ packId, revision }: { packId: string; revision?: string | null }) {
  const { t } = useTranslation();
  const [attempt, setAttempt] = useState(0);
  const queryKey = JSON.stringify([packId, revision, attempt]);
  const [response, setResponse] = useState<{
    key: string; value: StyleTextTrialReadiness | null; failed: boolean;
  } | null>(null);
  const current = response?.key === queryKey ? response : null;
  const readiness = current?.value ?? null;
  const failed = current?.failed ?? false;
  useEffect(() => {
    let disposed = false;
    let latest = 0;
    const refresh = () => {
      const request = ++latest;
      setResponse(null);
      void getStyleTextTrialReadiness(packId).then(value => {
        if (!disposed && request === latest) setResponse({ key: queryKey, value, failed: false });
      }).catch(() => {
        if (!disposed && request === latest) setResponse({ key: queryKey, value: null, failed: true });
      });
    };
    const onVisible = () => { if (document.visibilityState === 'visible') refresh(); };
    window.addEventListener(CREDENTIALS_STATUS_CHANGED_EVENT, refresh);
    window.addEventListener('focus', onVisible);
    document.addEventListener('visibilitychange', onVisible);
    refresh();
    return () => {
      disposed = true;
      window.removeEventListener(CREDENTIALS_STATUS_CHANGED_EVENT, refresh);
      window.removeEventListener('focus', onVisible);
      document.removeEventListener('visibilitychange', onVisible);
    };
  }, [packId, queryKey]);
  const key = failed ? 'serviceUnknown'
    : !readiness ? 'checking'
      : !readiness.available ? 'previewOnly'
        : !readiness.requiresTextService ? 'serviceFree'
          : readiness.textServiceConfigured ? 'serviceConfigured' : 'serviceMissing';
  return <span data-style-availability={packId} role="status" style={{ fontSize: 11.5, lineHeight: 1.5, color: 'var(--ol-ink-3)' }}>
    {t(`style.trial.${key}`)}
    {failed && <Btn size="sm" variant="ghost" onClick={() => setAttempt(value => value + 1)}>{t('common.retry')}</Btn>}
  </span>;
}
