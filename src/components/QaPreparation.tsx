import { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { getQaServiceStatus, qaOpenProviderSettings } from '../lib/ipc';
import { CREDENTIALS_STATUS_CHANGED_EVENT } from '../lib/credentialsStatusEvents';
import type { CredentialsStatus } from '../lib/types';

export function useQaPreparation() {
  const [credentials, setCredentials] = useState<CredentialsStatus | null>(null);
  const [failed, setFailed] = useState(false);
  const generation = useRef(0);
  const mounted = useRef(true);
  const refresh = useCallback(async () => {
    const current = ++generation.current;
    try {
      const next = await getQaServiceStatus();
      if (mounted.current && current === generation.current) { setCredentials(next); setFailed(false); }
    } catch {
      if (mounted.current && current === generation.current) { setCredentials(null); setFailed(true); }
    }
  }, []);
  useEffect(() => {
    mounted.current = true;
    void refresh();
    const changed = () => { void refresh(); };
    window.addEventListener(CREDENTIALS_STATUS_CHANGED_EVENT, changed);
    window.addEventListener('focus', changed);
    return () => {
      mounted.current = false; ++generation.current;
      window.removeEventListener(CREDENTIALS_STATUS_CHANGED_EVENT, changed);
      window.removeEventListener('focus', changed);
    };
  }, [refresh]);
  // QA specifically uses Volcengine, independently of ordinary dictation ASR.
  return { credentials, failed, refresh, ready: !!credentials?.volcengineConfigured && !!credentials?.llmConfigured };
}

export function QaPreparation({ preparation, compact = false }: { preparation: ReturnType<typeof useQaPreparation>; compact?: boolean }) {
  const { t } = useTranslation();
  const [opening, setOpening] = useState(false);
  const pending = useRef(false);
  const [openFailed, setOpenFailed] = useState(false);
  const open = async () => {
    if (pending.current) return;
    pending.current = true; setOpening(true); setOpenFailed(false);
    try { await qaOpenProviderSettings(); }
    catch { setOpenFailed(true); }
    finally { pending.current = false; setOpening(false); }
  };
  const { credentials, failed, ready } = preparation;
  const status = (configured: boolean | undefined) => t(!credentials ? failed ? 'qaPreparation.unknown' : 'qaPreparation.checking' : configured ? 'qaPreparation.configured' : 'qaPreparation.missing');
  return <section data-qa-preparation style={{ fontSize: 11, lineHeight: 1.6, ...(compact ? { maxHeight: 132, overflowY: 'auto' as const } : {}) }}>
    <div style={{ display: 'grid', gap: 5 }}>
    <div>{t('qaPreparation.speech')}: <strong>{status(credentials?.volcengineConfigured)}</strong></div>
    <div>{t('qaPreparation.answers')}: <strong>{status(credentials?.llmConfigured)}</strong></div>
    {!ready && <>
      <button data-qa-provider-settings onClick={() => void open()} disabled={opening}
        style={{ justifySelf: 'start', font: 'inherit', padding: '5px 9px', border: '1px solid var(--ol-line)', borderRadius: 6, color: 'var(--ol-ink-2)', background: 'transparent', cursor: 'pointer' }}>
        {t('qaPreparation.openSettings')}
      </button>
      {(failed || openFailed) && <span role="alert">{t(failed ? 'qaPreparation.readFailed' : 'qaPreparation.openFailed')}</span>}
      {failed && <button onClick={() => void preparation.refresh()}>{t('qaPreparation.retry')}</button>}
      <details data-qa-setup-help>
        <summary style={{ cursor: 'pointer', color: 'var(--ol-ink-3)' }}>{t('qaPreparation.helpTitle')}</summary>
        <p style={{ margin: '4px 0' }}>{t('qaPreparation.speechHelp')}</p>
        <p style={{ margin: '4px 0' }}>{t('qaPreparation.answersHelp')}</p>
        <p style={{ margin: '4px 0' }}>{t('qaPreparation.costs')}</p>
      </details>
    </>}
    </div>
  </section>;
}
