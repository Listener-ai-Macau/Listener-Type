import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { CREDENTIALS_STATUS_CHANGED_EVENT } from '../lib/credentialsStatusEvents';
import { getStyleTextTrialReadiness } from '../lib/ipc';
import type { StyleTextTrialReadiness } from '../lib/types';
import { Btn } from '../pages/_atoms';

/** Uses the same saved-style policy as the trial, including customized Raw packs. */
export function StyleTrialAvailability({ packId, revision }: { packId: string; revision?: string | null }) {
  const { t } = useTranslation();
  const [readiness, setReadiness] = useState<StyleTextTrialReadiness | null>(null);
  const [failed, setFailed] = useState(false);
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    let disposed = false;
    let latest = 0;
    const refresh = () => {
      const request = ++latest;
      setReadiness(null);
      setFailed(false);
      void getStyleTextTrialReadiness(packId).then(value => {
        if (!disposed && request === latest) setReadiness(value);
      }).catch(() => {
        if (!disposed && request === latest) setFailed(true);
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
  }, [packId, revision, attempt]);
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
