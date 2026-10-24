import { SaveTextDraft } from './SaveTextDraft';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { canInsertStyleTrialResult, cancelStyleTrialInsert, copyStyleTrialResult, getStyleTextTrialReadiness, insertStyleTrialResult, isTauri, tryStylePackText, tryTranslationText, tryLocalTranslationText } from '../lib/ipc';
import { detectOS } from './WindowChrome';
import { stylePackExamplePresentation, stylePackPresentation } from '../lib/stylePackPresentation';
import type { StylePack, StyleTextTrialReadiness, StyleTextTrialResult } from '../lib/types';
import { Btn } from '../pages/_atoms';
import './StyleTextTrial.css';
import { DeviceLayoutsDialog } from './DeviceLayoutsDialog';
import { writingDeviceKeyProfile } from '../lib/deviceKeyLayouts';
import { PoliteEditSuggestions } from './PoliteEditSuggestions';
import { ProviderSettingsDialog } from './ProviderSettingsDialog';
import { SUPPORTED_LANGUAGES } from '../lib/types';
import { LocalTranslationSetup } from './LocalTranslationSetup';
import { localTranslationErrorKey, localTranslationKey } from '../lib/localTranslation';
import type { TranslationEngine, LocalTranslationReadiness } from '../lib/localTranslation';

export function StyleTextTrial({ pack, packs, onSelectPack, extraStyleChoice, prepareShorter, translationTarget, onTranslationTargetChange, initialTranslationEngine, initialTranslationSource, draft, onDraftChange, onClose }: {
  pack: StylePack;
  packs?: StylePack[];
  onSelectPack?: (id: string) => void;
  extraStyleChoice?: { label: string; hint: string; pending: boolean; error: string; onChoose: () => void };
  prepareShorter?: () => Promise<StylePack>;
  translationTarget?: string;
  onTranslationTargetChange?: (language: string) => void;
  initialTranslationEngine?: TranslationEngine;
  initialTranslationSource?: string;
  draft: string;
  onDraftChange: (text: string) => void;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const [translationEngine, setTranslationEngine] = useState<TranslationEngine>(initialTranslationEngine ?? 'local');
  const [translationSource, setTranslationSource] = useState(initialTranslationSource ?? '');
  const [translationPreparing, setTranslationPreparing] = useState(false);
  const [offlineReadiness, setOfflineReadiness] = useState<LocalTranslationReadiness | null>(null);
  const offline = !!translationTarget && translationEngine === 'local';
  const offlineReady = offlineReadiness?.key === localTranslationKey(translationSource, translationTarget ?? '') && offlineReadiness.ready;
  const cacheKey = (id: string) => translationTarget ? JSON.stringify([id, translationEngine, translationSource]) : id;
  const styleChangePending = extraStyleChoice?.pending;
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
  const [shortening, setShortening] = useState(false);
  const [draftSaving, setDraftSaving] = useState(false);
  const [savedDraftSignature, setSavedDraftSignature] = useState('');
  const [keyPhrase, setKeyPhrase] = useState<string | null>(null);
  const [storedResult, setResult] = useState<StyleTextTrialResult | null>(null);
  const [resultSignature, setResultSignature] = useState('');
  const [editedOutput, setEditedOutput] = useState<string | null>(null);
  // Session-only comparisons, never History or persistent customer text storage.
  const completed = useRef(new Map<string, { signature: string; input: string; result: StyleTextTrialResult; edited: string | null }>());
  // A request can finish before its target style becomes selected. Carry that
  // exact fresh result through the selection, rather than calling it a restore
  // merely because it has just been added to the comparison cache.
  const freshSelectionResult = useRef<{ packId: string; signature: string; result: StyleTextTrialResult } | null>(null);
  const signature = JSON.stringify(translationTarget ? [draft, pack.prompt, translationEngine, translationSource] : [draft, pack.prompt]);
  const result = storedResult?.packId === pack.id && resultSignature === signature ? storedResult : null;
  const savedForPack = completed.current.get(cacheKey(pack.id));
  const earlier = !result && savedForPack && savedForPack.signature !== signature ? savedForPack : null;
  // Previous styles stay visibly labelled while a new style has no result yet.
  // They belong only to this exact input and unchanged saved prompt, never a new draft.
  const comparisons = (packs ?? [pack]).flatMap(option => {
    if (option.id === pack.id) return [];
    const saved = completed.current.get(option.id);
    return saved?.signature === JSON.stringify([draft, option.prompt])
      ? [{ pack: option, text: saved.edited ?? saved.result.outputText }] : [];
  });
  const [restored, setRestored] = useState(false);
  const [copying, setCopying] = useState(false);
  const [copiedResult, setCopiedResult] = useState<StyleTextTrialResult | null>(null);
  const [copyError, setCopyError] = useState(false);
  const [error, setError] = useState('');
  const [readinessAttempt, setReadinessAttempt] = useState(0);
  const readinessKey = JSON.stringify([pack.id, pack.baseMode, pack.prompt, readinessAttempt, translationTarget, translationEngine]);
  const [readinessResponse, setReadinessResponse] = useState<{
    key: string; value: StyleTextTrialReadiness | null; failed: boolean;
  } | null>(null);
  // A new selection renders before its effect resets state. Neither its privacy
  // notice nor submission permission may borrow the previous style's response.
  const currentReadiness = readinessResponse?.key === readinessKey ? readinessResponse : null;
  const readiness = currentReadiness?.value ?? null;
  const readinessError = currentReadiness?.failed ?? false;
  // All entry points keep this workspace mounted during service setup. A host
  // navigation callback must not discard paid results, edits, or pending work.
  const [providersOpen, setProvidersOpen] = useState(false);
  const deliveryRequest = useRef<string | null>(null);
  const [deliveryStarted, setDeliveryStarted] = useState<number | null>(null);
  const [deliverySeconds, setDeliverySeconds] = useState(5);
  const [deliveryMessage, setDeliveryMessage] = useState('');
  const delivering = deliveryStarted !== null;
  const resultOutput = editedOutput ?? result?.outputText ?? '';
  const savedCurrentDraft = savedDraftSignature === JSON.stringify([presentation.name, draft, resultOutput]);
  const canProcess = offline ? offlineReady : !!readiness?.available && (!readiness.requiresTextService || readiness.textServiceConfigured);

  useEffect(() => {
    ++generation.current;
    running.current = false;
    setPending(false);
    setShortening(false);
    const saved = completed.current.get(cacheKey(pack.id));
    const matching = saved?.signature === signature ? saved : null;
    const arriving = freshSelectionResult.current;
    freshSelectionResult.current = null;
    const newlyCompleted = arriving?.packId === pack.id && arriving.signature === signature && arriving.result === matching?.result;
    setResult(matching?.result ?? null); setResultSignature(signature); setEditedOutput(matching?.edited ?? null); setRestored(!!matching && !newlyCompleted);
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
    setReadinessResponse(null);
    if (offline) {
      setReadinessResponse({ key: readinessKey, value: { available: true, requiresTextService: false, textServiceConfigured: false }, failed: false });
      return;
    }
    void getStyleTextTrialReadiness(translationTarget ? 'builtin.formal' : pack.id).then(value => {
      if (current) setReadinessResponse({ key: readinessKey, value, failed: false });
    }).catch(() => {
      if (current) setReadinessResponse({ key: readinessKey, value: null, failed: true });
    });
    return () => { current = false; };
  }, [pack.id, readinessKey, translationTarget, offline]);

  useEffect(() => {
    const element = dialog.current;
    element?.showModal();
    return () => {
      generation.current += 1;
      if (deliveryRequest.current) void cancelStyleTrialInsert(deliveryRequest.current).catch(() => {});
      element?.close();
    };
  }, []);

  const runTrial = async (prepare?: () => Promise<StylePack>) => {
    if (draftSaving || translationPreparing || running.current || copyRunning.current || deliveryRequest.current || styleChangePending || !canProcess || !draft.trim()) return;
    if (prepare && !readiness?.textServiceConfigured) return;
    running.current = true;
    const request = ++generation.current;
    setPending(true);
    setShortening(!!prepare);
    freshSelectionResult.current = null;
    setResult(null);
    setEditedOutput(null);
    setRestored(false);
    setCopiedResult(null);
    setCopyError(false);
    setError('');
    setDeliveryMessage('');
    try {
      const target = prepare ? await prepare() : pack;
      // Closing the workspace while preparing must not start a paid request.
      if (generation.current !== request) return;
      const targetSignature = JSON.stringify(translationTarget ? [draft, target.prompt, translationEngine, translationSource] : [draft, target.prompt]);
      const cached = prepare ? completed.current.get(cacheKey(target.id)) : null;
      const matching = cached?.signature === targetSignature ? cached : null;
      const returned = matching?.result ?? await (translationTarget
        ? offline ? tryLocalTranslationText(draft, translationSource, translationTarget) : tryTranslationText(draft, translationTarget)
        : tryStylePackText(target.id, draft));
      // The backend receipt identifies the route; workspace identity also binds
      // the captured source/method signature, so late results cannot cross it.
      const value = offline ? { ...returned, packId: target.id, packName: target.name } : returned;
      if (generation.current === request) {
        completed.current.delete(cacheKey(target.id));
        completed.current.set(cacheKey(target.id), { signature: targetSignature, input: draft, result: value, edited: matching?.edited ?? null });
        if (completed.current.size > 8) completed.current.delete(completed.current.keys().next().value!);
        if (target.id !== pack.id && onSelectPack) {
          if (!matching) freshSelectionResult.current = { packId: target.id, signature: targetSignature, result: value };
          onSelectPack(target.id);
        } else {
          setResult(value); setResultSignature(targetSignature); setEditedOutput(matching?.edited ?? null); setRestored(!!matching);
        }
      }
    } catch (cause) {
      if (generation.current === request) {
        const message = String(cause instanceof Error ? cause.message : cause);
        const key = {
          style_trial_native_required: 'nativeRequired',
          style_trial_invalid_text: 'invalidText',
          style_trial_timeout: 'timeout',
        }[message];
        setResult(result); setResultSignature(resultSignature); setEditedOutput(editedOutput);
        setError(offline ? t(`offlineTranslation.${localTranslationErrorKey(cause)}`) : translationTarget && /status 401|AuthenticationError|Unauthorized|credentials were already rejected/i.test(message) ? t('translation.authFailed')
          : message === 'style_trial_prepare_shorter_failed' ? t('conciseDraft.failed')
          : key ? t(`style.trial.${key}`) : t('style.trial.failed', { error: message }));
      }
    } finally {
      if (generation.current === request) {
        running.current = false;
        setPending(false);
        setShortening(false);
      }
    }
  };

  const updateEditedOutput = (value: string) => {
    if (!result || copying || delivering || draftSaving) return;
    const edited = value === result.outputText ? null : value;
    setEditedOutput(edited);
    completed.current.set(cacheKey(pack.id), { signature, input: draft, result, edited });
    setCopiedResult(null); setCopyError(false); setDeliveryMessage('');
  };

  const copyResult = async () => {
    if (draftSaving || !result || !resultOutput.trim() || copyRunning.current || running.current || deliveryRequest.current) return;
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
    if (draftSaving || !result || !resultOutput.trim() || running.current || copyRunning.current || deliveryRequest.current) return;
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
    <><dialog
      ref={dialog}
      className={`ol-style-trial ol-thinscroll${result ? ' ol-style-trial-has-result' : ''}`}
      aria-labelledby="style-trial-title"
      aria-describedby="style-trial-description"
      onCancel={event => { event.preventDefault(); if (!draftSaving) onClose(); }}
    >
      <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'flex-start', gap: 16 }}>
        <h2 id="style-trial-title" style={{ margin: 0, fontSize: 21 }}>{translationTarget ? t('offlineTranslation.title', { language: translationTarget }) : t('style.trial.title', { name: presentation.name })}</h2>
        <Btn size="sm" variant="ghost" disabled={draftSaving} onClick={() => { if (!draftSaving) onClose(); }}>{t('common.close')}</Btn>
      </div>
      <p id="style-trial-description" className="ol-style-trial-hint" data-style-trial-service-notice>{t(offline ? 'offlineTranslation.localNotice' : readiness?.requiresTextService === false ? prepareShorter ? 'textWorkspace.localWithShorterNotice' : 'textWorkspace.localNotice' : 'textWorkspace.cloudNotice')}</p>
      {onTranslationTargetChange && <label className="ol-style-trial-picker" data-translation-output-choice>
        <span>{t('translation.outputLanguage')}</span>
        <select aria-label={t('translation.outputLanguage')} value={translationTarget ?? ''}
          disabled={pending || copying || delivering || draftSaving || styleChangePending || translationPreparing}
          onChange={event => onTranslationTargetChange(event.target.value)}>
          <option value="">{t('translation.originalOutput')}</option>
          {SUPPORTED_LANGUAGES.map(language => <option key={language} value={language}>{language}</option>)}
        </select>
      </label>}
      {translationTarget && <>
        <label className="ol-style-trial-picker">
          <span>{t('offlineTranslation.engine')}</span>
          <select aria-label={t('offlineTranslation.engine')} value={translationEngine}
            disabled={pending || copying || delivering || draftSaving || translationPreparing}
            onChange={event => setTranslationEngine(event.target.value as TranslationEngine)}>
            <option value="local">{t('offlineTranslation.local')}</option>
            <option value="cloud">{t('offlineTranslation.cloud')}</option>
          </select>
        </label>
        {offline && <LocalTranslationSetup source={translationSource} target={translationTarget}
          onSourceChange={setTranslationSource} disabled={pending || copying || delivering || draftSaving}
          onReadiness={setOfflineReadiness} onBusy={setTranslationPreparing} showNotice={false} />}
        <p className="ol-style-trial-hint" data-translation-trial-scope>{t('offlineTranslation.trialOnly')}</p>
      </>}
      {packs && onSelectPack && <label className="ol-style-trial-picker">
        <span>{t('style.trial.styleLabel')}</span>
        <select aria-label={t('style.trial.styleLabel')} value={pack.id} disabled={pending || copying || delivering || draftSaving || styleChangePending}
          onChange={event => onSelectPack(event.target.value)}>
          {packs.map(option => <option key={option.id} value={option.id}>{stylePackPresentation(option, t).name}</option>)}
        </select>
      </label>}
      {!translationTarget && <p className="ol-style-trial-hint ol-style-trial-purpose" data-style-trial-purpose>{presentation.description}</p>}
      {(!readiness || !readiness.available || (readiness.requiresTextService && !readiness.textServiceConfigured) || readinessError) && <div className="ol-style-trial-availability">
      {readiness?.available === false && <p role="status" className="ol-style-trial-hint">{t('style.trial.nativeRequired')}</p>}
      {readiness?.available && readiness.requiresTextService && !readiness.textServiceConfigured && <p role="status" className="ol-style-trial-hint">{t('style.trial.needsService')}</p>}
      {!readiness && !readinessError && <p role="status" className="ol-style-trial-hint">{t('style.trial.checking')}</p>}
      {readinessError && <p role="alert" className="ol-style-trial-hint">{t('style.trial.readinessFailed')} <Btn size="sm" onClick={() => setReadinessAttempt(value => value + 1)}>{t('common.retry')}</Btn></p>}
      </div>}
      <div className="ol-style-trial-workspace">
      <div className="ol-style-trial-draft">
      <label htmlFor="style-trial-input" style={{ display: 'block', fontWeight: 600, marginBottom: 8 }}>
        {t('style.trial.input')}
      </label>
      <textarea
        id="style-trial-input"
        className="ol-style-trial-input ol-thinscroll"
        value={draft}
        maxLength={4000}
        rows={6}
        // Readiness gates submission, not drafting: checking/failure must not discard typing.
        disabled={pending || copying || delivering || draftSaving || styleChangePending}
        placeholder={t('style.trial.placeholder')}
        onChange={event => { onDraftChange(event.target.value); setResult(null); setError(''); setCopiedResult(null); setCopyError(false); }}
      />
      <div style={{ display: 'flex', flexWrap: 'wrap', gap: 12, alignItems: 'center', marginTop: 12 }}>
        <Btn variant="blue" disabled={pending || copying || delivering || draftSaving || styleChangePending || !canProcess || !draft.trim()} onClick={() => void runTrial()}>
          {t(pending ? 'style.trial.running' : translationTarget ? 'offlineTranslation.run' : 'style.trial.run')}
        </Btn>
        {prepareShorter && <Btn variant="ghost" disabled={pending || copying || delivering || draftSaving || styleChangePending || !canProcess || !readiness?.textServiceConfigured || !draft.trim()}
          onClick={() => void runTrial(prepareShorter)}>{t(shortening ? 'conciseDraft.running' : 'conciseDraft.run')}</Btn>}
        {!offline && readiness?.available && <Btn variant="ghost" disabled={draftSaving} onClick={() => setProvidersOpen(true)}>{t('style.trial.providers')}</Btn>}
        {readiness?.available === false && <a href="https://github.com/Listener-ai-Macau/Listener-Type/releases/tag/v1.0.6-rc1" target="_blank" rel="noreferrer noopener" style={{ color: 'var(--ol-blue)', fontSize: 13 }}>{t('style.trial.download')}</a>}
      </div>
      {pending && <p role="status" className="ol-style-trial-hint" data-style-trial-pending>
        {t(offline ? 'offlineTranslation.pending' : !shortening && readiness?.requiresTextService === false ? 'textWorkspace.pendingLocal' : 'style.trial.pending')}
      </p>}
      {error && <p role="alert" style={{ color: 'var(--ol-err)', whiteSpace: 'pre-wrap', overflowWrap: 'anywhere', lineHeight: 1.6 }}>{error}</p>}
      </div>
      <div className="ol-style-trial-outputs">
      {result && (
        <section ref={resultSection} className="ol-style-trial-result" aria-label={t('style.trial.output')} aria-live="polite">
          <div className="ol-style-trial-result-heading">
            <div style={{ fontWeight: 600 }}>{t('style.trial.output')}</div>
            <span className="ol-style-trial-result-meta" title={t(editedOutput !== null ? 'style.trial.editedResult' : offline ? 'offlineTranslation.result' : result.usedTextService ? 'style.trial.serviceResult' : 'style.trial.rawResult', { seconds: (result.elapsedMs / 1000).toFixed(1) })}>
              {t(editedOutput !== null ? 'textWorkspace.editedResult' : offline ? 'offlineTranslation.result' : result.usedTextService ? 'textWorkspace.serviceResult' : 'textWorkspace.localResult', { seconds: (result.elapsedMs / 1000).toFixed(1) })}
            </span>
          </div>
          {result.usedTextService && result.outputText === draft && editedOutput === null && <p role="note" className="ol-style-trial-hint">{t('politeEdits.unchangedResult')}</p>}
          <textarea className="ol-style-trial-result-body ol-style-trial-result-editor ol-thinscroll"
            aria-label={t('style.trial.resultInput')} value={resultOutput} rows={8} maxLength={100000}
            disabled={copying || delivering || draftSaving}
            onChange={event => {
              if (draftSaving || copyRunning.current || deliveryRequest.current) return;
              const value = event.target.value;
              updateEditedOutput(value);
            }} />
          <div className="ol-style-trial-primary-actions">
            <Btn size="sm" variant="blue" disabled={copying || delivering || draftSaving || !resultOutput.trim()} onClick={() => void copyResult()}>
              {t(copying ? 'style.trial.copying' : copiedResult === result ? 'style.trial.copied' : 'style.trial.copyResult')}
            </Btn>
            <SaveTextDraft compact name={presentation.name} original={draft} text={resultOutput} disabled={copying || delivering} onBusy={setDraftSaving}
              onUnsaved={() => setSavedDraftSignature('')}
              onSaved={text => setSavedDraftSignature(JSON.stringify([presentation.name, draft, text]))} />
            {(!offline || translationTarget === 'English') && <PoliteEditSuggestions text={resultOutput} localOnly={offline} cloudConfigured={!offline && readiness?.textServiceConfigured === true}
              disabled={copying || delivering || draftSaving} onApply={updateEditedOutput} />}
          </div>
          <p role="note" className="ol-style-trial-hint" data-style-trial-retention>{t(savedCurrentDraft ? isTauri ? 'textWorkspace.saved' : 'textDraft.browserSaved' : isTauri ? 'textWorkspace.unsaved' : 'textWorkspace.browserUnsaved')}</p>
          {restored && <p role="note" className="ol-style-trial-hint" data-style-trial-restored>{t('style.trial.restoredResult')}</p>}
          <details className="ol-style-trial-details ol-style-trial-more-actions">
            <summary>{t('style.trial.moreActions')}</summary>
            <div className="ol-style-trial-secondary-actions">
              <Btn size="sm" variant="ghost" disabled={copying || delivering || draftSaving || !resultOutput.trim()} onClick={() => setKeyPhrase(resultOutput)}>{t('keyPhrase.open')}</Btn>
              {canInsertStyleTrialResult && <Btn size="sm" variant="ghost" disabled={copying || delivering || draftSaving || !resultOutput.trim()} onClick={() => void insertElsewhere()}>{t('style.trial.insertElsewhere')}</Btn>}
            </div>
            {canInsertStyleTrialResult && <p className="ol-style-trial-hint">{t('style.trial.insertHint')}</p>}
          </details>
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
        <p className="ol-style-trial-hint">{t(pending ? 'style.trial.running' : 'style.trial.resultPlaceholder')}</p>
      </div>}
      {earlier && <section className="ol-style-trial-comparisons" aria-label={t('earlierTrial.title')} data-style-trial-earlier-result>
        <h3>{t('earlierTrial.title')}</h3>
        <p className="ol-style-trial-hint">{t('earlierTrial.hint')}</p>
        <pre>{earlier.edited ?? earlier.result.outputText}</pre>
        <details className="ol-style-trial-details"><summary>{t('earlierTrial.input')}</summary><pre>{earlier.input}</pre></details>
      </section>}
      {comparisons.length > 0 && <section className="ol-style-trial-comparisons" aria-label={t('styleComparisons.title')} data-style-trial-comparisons>
        <h3>{t('styleComparisons.title')}</h3>
        <p className="ol-style-trial-hint">{t('styleComparisons.hint')}</p>
        {comparisons.map(previous => <article key={previous.pack.id}>
          <div className="ol-style-trial-comparison-heading">
            <h4>{stylePackPresentation(previous.pack, t).name}</h4>
            {onSelectPack && <Btn size="sm" variant="ghost" disabled={pending || copying || delivering || draftSaving || styleChangePending}
              onClick={() => onSelectPack(previous.pack.id)}>{t('styleComparisons.open', {name:stylePackPresentation(previous.pack, t).name})}</Btn>}
          </div>
          <pre>{previous.text}</pre>
        </article>)}
      </section>}
      </div>
      </div>
      {extraStyleChoice && <div className="ol-style-trial-alternative">
        <Btn size="sm" variant="ghost" disabled={pending || copying || delivering || draftSaving || extraStyleChoice.pending} onClick={extraStyleChoice.onChoose}>
          {extraStyleChoice.pending ? t('style.everydayList.adding') : extraStyleChoice.label}
        </Btn>
        <p className="ol-style-trial-hint">{extraStyleChoice.hint}</p>
        {extraStyleChoice.error && <p role="alert" className="ol-style-trial-hint">{extraStyleChoice.error}</p>}
      </div>}
      <details className="ol-style-trial-details">
        <summary>{t('style.trial.detailsSummary')}</summary>
        <p className="ol-style-trial-hint">{t('style.trial.shortNotice')}</p>
        <p className="ol-style-trial-hint">{t('style.trial.description')}</p>
        <p className="ol-style-trial-hint">{t('style.trial.serviceNotice')}</p>
        {prepareShorter && <p className="ol-style-trial-hint">{t('conciseDraft.runHint')}</p>}
        <p className="ol-style-trial-hint">{t('style.trial.editHint')}</p>
        <p className="ol-style-trial-hint">{t('textDraft.retention')}</p>
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
      {keyPhrase !== null && <DeviceLayoutsDialog rememberSelection initialProfile={writingDeviceKeyProfile()} incomingText={keyPhrase} onLeaveForPage={onClose} onClose={() => setKeyPhrase(null)} />}
    </dialog>
    {providersOpen && <ProviderSettingsDialog kind="text" onClose={() => {
      setProvidersOpen(false); setReadinessAttempt(value => value + 1);
    }} />}</>
  );
}
