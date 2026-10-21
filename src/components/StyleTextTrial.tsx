import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { canInsertStyleTrialResult, cancelStyleTrialInsert, copyStyleTrialResult, getStyleTextTrialReadiness, insertStyleTrialResult, tryStylePackText } from '../lib/ipc';
import { detectOS } from './WindowChrome';
import { stylePackExamplePresentation, stylePackPresentation } from '../lib/stylePackPresentation';
import type { StylePack, StyleTextTrialReadiness, StyleTextTrialResult } from '../lib/types';
import { Btn } from '../pages/_atoms';
import './StyleTextTrial.css';

export function StyleTextTrial({ pack, packs, onSelectPack, extraStyleChoice, draft, onDraftChange, onClose, onOpenProviders }: {
  pack: StylePack;
  packs?: StylePack[];
  onSelectPack?: (id: string) => void;
  extraStyleChoice?: { label: string; hint: string; pending: boolean; error: string; onChoose: () => void };
  draft: string;
  onDraftChange: (text: string) => void;
  onClose: () => void;
  onOpenProviders?: () => void;
}) {
  const { t } = useTranslation();
  const presentation = stylePackPresentation(pack, t);
  const savedExamples = pack.examples
    .filter(example => example.input.trim() && example.output.trim())
    .map(example => stylePackExamplePresentation(pack, example, t));
  const dialog = useRef<HTMLDialogElement>(null);
  const resultSection = useRef<HTMLElement>(null);
  const generation = useRef(0);
  const running = useRef(false);
  const copyRunning = useRef(false);
  const [pending, setPending] = useState(false);
  const [storedResult, setResult] = useState<StyleTextTrialResult | null>(null);
  const [resultSignature, setResultSignature] = useState('');
  const [editedOutput, setEditedOutput] = useState<string | null>(null);
  // Session-only comparisons, never History or persistent customer text storage.
  const completed = useRef(new Map<string, { signature: string; result: StyleTextTrialResult; edited: string | null }>());
  const signature = JSON.stringify([draft, pack.prompt]);
  const result = storedResult?.packId === pack.id && resultSignature === signature ? storedResult : null;
  const [restored, setRestored] = useState(false);
  const [copying, setCopying] = useState(false);
  const [copiedResult, setCopiedResult] = useState<StyleTextTrialResult | null>(null);
  const [copyError, setCopyError] = useState(false);
  const [error, setError] = useState('');
  const [readiness, setReadiness] = useState<StyleTextTrialReadiness | null>(null);
  const [readinessError, setReadinessError] = useState(false);
  const [readinessAttempt, setReadinessAttempt] = useState(0);
  const deliveryRequest = useRef<string | null>(null);
  const [deliveryStarted, setDeliveryStarted] = useState<number | null>(null);
  const [deliverySeconds, setDeliverySeconds] = useState(5);
  const [deliveryMessage, setDeliveryMessage] = useState('');
  const delivering = deliveryStarted !== null;
  const resultOutput = editedOutput ?? result?.outputText ?? '';
  const canProcess = !!readiness?.available && (!readiness.requiresTextService || readiness.textServiceConfigured);

  useEffect(() => {
    ++generation.current;
    running.current = false;
    setPending(false);
    const saved = completed.current.get(pack.id);
    const matching = saved?.signature === signature ? saved : null;
    setResult(matching?.result ?? null); setResultSignature(signature); setEditedOutput(matching?.edited ?? null); setRestored(!!matching);
    setCopiedResult(null); setCopyError(false); setError(''); setDeliveryMessage('');
    if (deliveryRequest.current) {
      void cancelStyleTrialInsert(deliveryRequest.current).catch(() => {});
      deliveryRequest.current = null; setDeliveryStarted(null);
    }
  }, [pack.id, signature]);

  useEffect(() => {
    // Keep a new result reachable in a short/narrow dialog without moving focus.
    if (result && window.matchMedia('(max-width: 800px)').matches) {
      resultSection.current?.scrollIntoView({ block: 'nearest' });
    }
  }, [result]);

  useEffect(() => {
    let current = true;
    setReadiness(null);
    setReadinessError(false);
    void getStyleTextTrialReadiness(pack.id).then(value => {
      if (current) setReadiness(value);
    }).catch(() => {
      if (current) setReadinessError(true);
    });
    return () => { current = false; };
  }, [pack.id, readinessAttempt]);

  useEffect(() => {
    const element = dialog.current;
    element?.showModal();
    return () => {
      generation.current += 1;
      if (deliveryRequest.current) void cancelStyleTrialInsert(deliveryRequest.current).catch(() => {});
      element?.close();
    };
  }, []);

  const runTrial = async () => {
    if (running.current || copyRunning.current || deliveryRequest.current || extraStyleChoice?.pending || !canProcess || !draft.trim()) return;
    running.current = true;
    const request = ++generation.current;
    setPending(true);
    setResult(null);
    setEditedOutput(null);
    setRestored(false);
    setCopiedResult(null);
    setCopyError(false);
    setError('');
    setDeliveryMessage('');
    try {
      const value = await tryStylePackText(pack.id, draft);
      if (generation.current === request) {
        completed.current.delete(pack.id);
        completed.current.set(pack.id, { signature, result: value, edited: null });
        if (completed.current.size > 8) completed.current.delete(completed.current.keys().next().value!);
        setResult(value); setResultSignature(signature);
      }
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

  const copyResult = async () => {
    if (!result || !resultOutput.trim() || copyRunning.current || running.current || deliveryRequest.current) return;
    const selectedResult = result;
    const request = generation.current;
    copyRunning.current = true;
    setCopying(true);
    setCopiedResult(null);
    setCopyError(false);
    try {
      await copyStyleTrialResult(resultOutput, detectOS() === 'win');
      if (generation.current === request) setCopiedResult(selectedResult);
    } catch {
      if (generation.current === request) setCopyError(true);
    } finally {
      copyRunning.current = false;
      if (generation.current === request) setCopying(false);
    }
  };

  useEffect(() => {
    if (deliveryStarted === null) return;
    const timer = window.setInterval(() => setDeliverySeconds(Math.max(0, Math.ceil((5000 - (Date.now() - deliveryStarted)) / 1000))), 100);
    return () => window.clearInterval(timer);
  }, [deliveryStarted]);

  const insertElsewhere = async () => {
    if (!result || !resultOutput.trim() || running.current || copyRunning.current || deliveryRequest.current) return;
    const id = crypto.randomUUID();
    deliveryRequest.current = id;
    setDeliveryMessage('');
    setDeliverySeconds(5);
    setDeliveryStarted(Date.now());
    try {
      const status = await insertStyleTrialResult(id, resultOutput);
      if (deliveryRequest.current === id) setDeliveryMessage(t(status === 'confirmed' ? 'style.trial.insertConfirmed' : 'style.trial.insertSent'));
    } catch (cause) {
      if (deliveryRequest.current === id) {
        const message = String(cause instanceof Error ? cause.message : cause);
        setDeliveryMessage(t(message === 'style_insert_recording' ? 'style.trial.insertRecording' : 'style.trial.insertFailed'));
      }
    } finally {
      if (deliveryRequest.current === id) { deliveryRequest.current = null; setDeliveryStarted(null); }
    }
  };

  const cancelInsert = async () => {
    const id = deliveryRequest.current;
    if (!id) return;
    try {
      const cancelled = await cancelStyleTrialInsert(id);
      if (cancelled && deliveryRequest.current === id) {
        deliveryRequest.current = null;
        setDeliveryStarted(null);
        setDeliveryMessage(t('style.trial.insertCancelled'));
      }
    } catch { /* The pending command still reports its actual outcome. */ }
  };

  return (
    <dialog
      ref={dialog}
      className={`ol-style-trial ol-thinscroll${result ? ' ol-style-trial-has-result' : ''}`}
      aria-labelledby="style-trial-title"
      aria-describedby="style-trial-description"
      onCancel={event => { event.preventDefault(); onClose(); }}
    >
      <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'flex-start', gap: 16 }}>
        <h2 id="style-trial-title" style={{ margin: 0, fontSize: 21 }}>{t('style.trial.title', { name: presentation.name })}</h2>
        <Btn size="sm" variant="ghost" onClick={onClose}>{t('common.close')}</Btn>
      </div>
      <p id="style-trial-description" className="ol-style-trial-hint">{t('style.trial.shortNotice')}</p>
      {packs && onSelectPack && <label className="ol-style-trial-picker">
        <span>{t('style.trial.styleLabel')}</span>
        <select aria-label={t('style.trial.styleLabel')} value={pack.id} disabled={pending || copying || delivering || extraStyleChoice?.pending}
          onChange={event => onSelectPack(event.target.value)}>
          {packs.map(option => <option key={option.id} value={option.id}>{stylePackPresentation(option, t).name}</option>)}
        </select>
      </label>}
      <p className="ol-style-trial-hint ol-style-trial-purpose" data-style-trial-purpose>{presentation.description}</p>
      <div className="ol-style-trial-availability">
      {readiness?.available === false && <p role="status" className="ol-style-trial-hint">{t('style.trial.nativeRequired')}</p>}
      {readiness?.available && readiness.requiresTextService && !readiness.textServiceConfigured && <p role="status" className="ol-style-trial-hint">{t('style.trial.needsService')}</p>}
      {!readiness && !readinessError && <p role="status" className="ol-style-trial-hint">{t('style.trial.checking')}</p>}
      {readinessError && <p role="alert" className="ol-style-trial-hint">{t('style.trial.readinessFailed')} <Btn size="sm" onClick={() => setReadinessAttempt(value => value + 1)}>{t('common.retry')}</Btn></p>}
      </div>
      <div className="ol-style-trial-workspace">
      <div className="ol-style-trial-draft">
      <label htmlFor="style-trial-input" style={{ display: 'block', fontWeight: 600, marginBottom: 8 }}>
        {t('style.trial.input')}
      </label>
      <textarea
        id="style-trial-input"
        value={draft}
        maxLength={4000}
        rows={6}
        // Readiness gates submission, not drafting: checking/failure must not discard typing.
        disabled={pending || copying || delivering || extraStyleChoice?.pending}
        placeholder={t('style.trial.placeholder')}
        onChange={event => { onDraftChange(event.target.value); setResult(null); setError(''); setCopiedResult(null); setCopyError(false); }}
        style={{ width: '100%', boxSizing: 'border-box', resize: 'vertical', minHeight: 110, padding: 12, borderRadius: 12, border: '1px solid var(--ol-line-strong)', color: 'var(--ol-ink)', background: 'var(--ol-surface-2)', font: 'inherit', lineHeight: 1.65 }}
      />
      <div style={{ display: 'flex', flexWrap: 'wrap', gap: 12, alignItems: 'center', marginTop: 12 }}>
        <Btn variant="blue" disabled={pending || copying || delivering || extraStyleChoice?.pending || !canProcess || !draft.trim()} onClick={() => void runTrial()}>
          {t(pending ? 'style.trial.running' : 'style.trial.run')}
        </Btn>
        {readiness?.available && onOpenProviders && <Btn variant="ghost" onClick={onOpenProviders}>{t('style.trial.providers')}</Btn>}
        {readiness?.available === false && <a href="https://github.com/Listener-ai-Macau/Listener-Type/releases/tag/v1.0.6-rc1" target="_blank" rel="noreferrer noopener" style={{ color: 'var(--ol-blue)', fontSize: 13 }}>{t('style.trial.download')}</a>}
      </div>
      {pending && <p role="status" className="ol-style-trial-hint">{t('style.trial.pending')}</p>}
      {error && <p role="alert" style={{ color: 'var(--ol-err)', whiteSpace: 'pre-wrap', overflowWrap: 'anywhere', lineHeight: 1.6 }}>{error}</p>}
      </div>
      {result && (
        <section ref={resultSection} className="ol-style-trial-result" aria-label={t('style.trial.output')} aria-live="polite">
          <div style={{ display: 'flex', gap: 12, alignItems: 'center', justifyContent: 'space-between', flexWrap: 'wrap' }}>
            <div style={{ fontWeight: 600 }}>{t('style.trial.output')}</div>
            <div style={{ display: 'flex', flexWrap: 'wrap', gap: 8 }}>
            <Btn size="sm" variant="ghost" disabled={copying || delivering || !resultOutput.trim()} onClick={() => void copyResult()}>
              {t(copying ? 'style.trial.copying' : copiedResult === result ? 'style.trial.copied' : 'style.trial.copyResult')}
            </Btn>
            {canInsertStyleTrialResult && <Btn size="sm" variant="blue" disabled={copying || delivering || !resultOutput.trim()} onClick={() => void insertElsewhere()}>{t('style.trial.insertElsewhere')}</Btn>}
            </div>
          </div>
          <p role="note" className="ol-style-trial-hint" data-style-trial-retention>{t('style.trial.keepResult')}</p>
          {restored && <p role="note" className="ol-style-trial-hint" data-style-trial-restored>{t('style.trial.restoredResult')}</p>}
          <p className="ol-style-trial-hint">{editedOutput !== null ? t('style.trial.editedResult') : t(result.usedTextService ? 'style.trial.serviceResult' : 'style.trial.rawResult', { seconds: (result.elapsedMs / 1000).toFixed(1) })}</p>
          <textarea className="ol-style-trial-result-body ol-style-trial-result-editor ol-thinscroll"
            aria-label={t('style.trial.resultInput')} value={resultOutput} rows={8} maxLength={100000}
            disabled={copying || delivering}
            onChange={event => {
              if (copyRunning.current || deliveryRequest.current) return;
              const value = event.target.value;
              const edited = value === result.outputText ? null : value;
              setEditedOutput(edited);
              completed.current.set(pack.id, { signature, result, edited });
              setCopiedResult(null); setCopyError(false); setDeliveryMessage('');
            }} />
          <p className="ol-style-trial-hint">{t('style.trial.editHint')}</p>
          {canInsertStyleTrialResult && <p className="ol-style-trial-hint">{t('style.trial.insertHint')}</p>}
          {delivering && <div role="status" className="ol-style-trial-hint">
            {t(deliverySeconds > 0 ? 'style.trial.insertCountdown' : 'style.trial.insertWorking', { seconds: deliverySeconds })}
            {' '}<Btn size="sm" variant="ghost" onClick={() => void cancelInsert()}>{t('common.cancel')}</Btn>
          </div>}
          {deliveryMessage && <p role="status" className="ol-style-trial-hint">{deliveryMessage}</p>}
          {copiedResult === result && <p role="status" className="ol-style-trial-hint">{t('style.trial.copied')}</p>}
          {copyError && <p role="alert" className="ol-style-trial-hint">{t('style.trial.copyFailed')}</p>}
        </section>
      )}
      {!result && <div className="ol-style-trial-placeholder" data-style-trial-placeholder>
        <h3>{t('style.trial.output')}</h3>
        <p className="ol-style-trial-hint">{t('style.trial.resultPlaceholder')}</p>
      </div>}
      </div>
      {extraStyleChoice && <div className="ol-style-trial-alternative">
        <Btn size="sm" variant="ghost" disabled={pending || copying || delivering || extraStyleChoice.pending} onClick={extraStyleChoice.onChoose}>
          {extraStyleChoice.pending ? t('style.everydayList.adding') : extraStyleChoice.label}
        </Btn>
        <p className="ol-style-trial-hint">{extraStyleChoice.hint}</p>
        {extraStyleChoice.error && <p role="alert" className="ol-style-trial-hint">{extraStyleChoice.error}</p>}
      </div>}
      <details className="ol-style-trial-details">
        <summary>{t('style.trial.detailsSummary')}</summary>
        <p className="ol-style-trial-hint">{t('style.trial.description')}</p>
        <p className="ol-style-trial-hint">{t('style.trial.serviceNotice')}</p>
      </details>
      {readiness?.requiresTextService && savedExamples.length > 0 && (
        <details className="ol-style-saved-examples" data-style-saved-examples>
          <summary>{t('style.trial.exampleSummary')}</summary>
          <p className="ol-style-trial-hint" role="note">{t('style.trial.exampleNotice')}</p>
          {savedExamples.map((example, index) => (
            <article key={index} className="ol-style-saved-example">
              <div style={{ fontWeight: 600 }}>{example.title || t('style.trial.exampleUntitled', { number: index + 1 })}</div>
              <div className="ol-style-trial-hint">{t('style.trial.exampleInput')}</div>
              <pre>{example.input}</pre>
              <div className="ol-style-trial-hint">{t('style.trial.exampleOutput')}</div>
              <pre>{example.output}</pre>
            </article>
          ))}
        </details>
      )}
    </dialog>
  );
}
