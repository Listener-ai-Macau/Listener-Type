import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { tryStylePackText } from '../lib/ipc';
import type { StylePack, StyleTextTrialResult } from '../lib/types';
import { Btn } from '../pages/_atoms';
import './StyleTextTrial.css';

export function StyleTextTrial({ pack, draft, onDraftChange, onClose, onOpenProviders }: {
  pack: StylePack;
  draft: string;
  onDraftChange: (text: string) => void;
  onClose: () => void;
  onOpenProviders?: () => void;
}) {
  const { t } = useTranslation();
  const dialog = useRef<HTMLDialogElement>(null);
  const generation = useRef(0);
  const running = useRef(false);
  const [pending, setPending] = useState(false);
  const [result, setResult] = useState<StyleTextTrialResult | null>(null);
  const [error, setError] = useState('');

  useEffect(() => {
    const element = dialog.current;
    element?.showModal();
    return () => {
      generation.current += 1;
      element?.close();
    };
  }, []);

  const runTrial = async () => {
    if (running.current || !draft.trim()) return;
    running.current = true;
    const request = ++generation.current;
    setPending(true);
    setResult(null);
    setError('');
    try {
      const value = await tryStylePackText(pack.id, draft);
      if (generation.current === request) setResult(value);
    } catch (cause) {
      if (generation.current === request) {
        const message = String(cause instanceof Error ? cause.message : cause);
        const key = {
          style_trial_native_required: 'nativeRequired',
          style_trial_invalid_text: 'invalidText',
          style_trial_timeout: 'timeout',
        }[message];
        setError(key ? t(`style.trial.${key}`) : t('style.trial.failed', { error: message }));
      }
    } finally {
      if (generation.current === request) {
        running.current = false;
        setPending(false);
      }
    }
  };

  return (
    <dialog
      ref={dialog}
      className="ol-style-trial ol-thinscroll"
      aria-labelledby="style-trial-title"
      aria-describedby="style-trial-description"
      onCancel={event => { event.preventDefault(); onClose(); }}
    >
      <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'flex-start', gap: 16 }}>
        <h2 id="style-trial-title" style={{ margin: 0, fontSize: 21 }}>{t('style.trial.title', { name: pack.name })}</h2>
        <Btn size="sm" variant="ghost" onClick={onClose}>{t('common.close')}</Btn>
      </div>
      <p id="style-trial-description" className="ol-style-trial-hint">{t('style.trial.description')}</p>
      <p className="ol-style-trial-hint">{t('style.trial.serviceNotice')}</p>
      <label htmlFor="style-trial-input" style={{ display: 'block', fontWeight: 600, marginTop: 20, marginBottom: 8 }}>
        {t('style.trial.input')}
      </label>
      <textarea
        id="style-trial-input"
        value={draft}
        maxLength={4000}
        rows={5}
        disabled={pending}
        placeholder={t('style.trial.placeholder')}
        onChange={event => { onDraftChange(event.target.value); setResult(null); setError(''); }}
        style={{ width: '100%', boxSizing: 'border-box', resize: 'vertical', minHeight: 110, padding: 12, borderRadius: 12, border: '1px solid var(--ol-line-strong)', color: 'var(--ol-ink)', background: 'var(--ol-surface-2)', font: 'inherit', lineHeight: 1.65 }}
      />
      <div style={{ display: 'flex', flexWrap: 'wrap', gap: 12, alignItems: 'center', marginTop: 12 }}>
        <Btn variant="blue" disabled={pending || !draft.trim()} onClick={() => void runTrial()}>
          {t(pending ? 'style.trial.running' : 'style.trial.run')}
        </Btn>
        {onOpenProviders && <Btn variant="ghost" onClick={onOpenProviders}>{t('style.trial.providers')}</Btn>}
      </div>
      {pending && <p role="status" className="ol-style-trial-hint">{t('style.trial.pending')}</p>}
      {error && <p role="alert" style={{ color: 'var(--ol-err)', whiteSpace: 'pre-wrap', overflowWrap: 'anywhere', lineHeight: 1.6 }}>{error}</p>}
      {result && (
        <section aria-label={t('style.trial.output')} aria-live="polite" style={{ marginTop: 20 }}>
          <div style={{ fontWeight: 600 }}>{t('style.trial.output')}</div>
          <p className="ol-style-trial-hint">{t(result.usedTextService ? 'style.trial.serviceResult' : 'style.trial.rawResult', { seconds: (result.elapsedMs / 1000).toFixed(1) })}</p>
          <div style={{ padding: 14, borderRadius: 12, background: 'var(--ol-blue-soft)', whiteSpace: 'pre-wrap', overflowWrap: 'anywhere', lineHeight: 1.75, userSelect: 'text' }}>{result.outputText}</div>
        </section>
      )}
    </dialog>
  );
}
