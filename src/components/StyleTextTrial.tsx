import { SaveTextDraft } from './SaveTextDraft';
import { useEffect, useId, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { canInsertStyleTrialResult, cancelStyleTrialInsert, copyStyleTrialResult, getStyleTextTrialReadiness, insertStyleTrialResult, isTauri, tryStylePackText, tryTranslationText, tryLocalTranslationText, cancelLocalTranslation } from '../lib/ipc';
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
import { canSuggestChineseSource, localTargetCode, localTranslationErrorKey, localTranslationKey, translationLanguageLabel } from '../lib/localTranslation';
import type { TranslationEngine, LocalTranslationReadiness, LocalTranslationQuality } from '../lib/localTranslation';
import { SavedTextDraftsDialog } from './SavedTextDraftsDialog';
import { preserveBufferedSourceFacts, preserveTranslatedSourceFacts } from '../lib/bufferedSourceFacts';
import { isMessageDraftStyle } from '../lib/messageDraftStyle';

function withDraftOriginName(origin: string, suffix: string): string {
  const room = Math.max(0, 80 - Array.from(suffix).length - 3);
  const prefix = Array.from(origin.trim()).slice(0, room).join('').trimEnd();
  return prefix ? `${prefix} · ${suffix}` : suffix;
}

export function StyleTextTrial({ pack, packs, onSelectPack, extraStyleChoice, draftingChoice, prepareShorter, translationTarget, onTranslationTargetChange, onRefineTranslation, initialTranslationEngine, initialTranslationSource, initialTranslationQuality, inputOriginName, inputOriginal, suspended = false, onOpenSavedDraft, draft, onDraftChange, onClose }: {
  pack: StylePack;
  packs?: StylePack[];
  onSelectPack?: (id: string) => void;
  extraStyleChoice?: { label: string; hint: string; pending: boolean; error: string; onChoose: () => void };
  draftingChoice?: { label: string; hint: string; pending: boolean; error: string; onChoose: () => void };
  prepareShorter?: () => Promise<StylePack>;
  translationTarget?: string;
  onTranslationTargetChange?: (language: string) => void;
  onRefineTranslation?: () => void;
  initialTranslationEngine?: TranslationEngine;
  initialTranslationSource?: string;
  initialTranslationQuality?: LocalTranslationQuality;
  inputOriginName?: string;
  inputOriginal?: string;
  suspended?: boolean;
  onOpenSavedDraft?: (id: string, onUnavailable: (id: string) => void) => void;
  draft: string;
  onDraftChange: (text: string) => void;
  onClose: () => void;
}) {
  const { t, i18n } = useTranslation();
  const workspaceId = useId();
  const [draftNameEdit, setDraftNameEdit] = useState<string | null>(null);
  const [localLibrary, setLocalLibrary] = useState<{ id: string; onUnavailable: (id: string) => void } | null>(null);
  const modalSuspended = suspended || localLibrary !== null;
  const [translationOriginName, setTranslationOriginName] = useState(inputOriginName ?? '');
  const [translationEngine, setTranslationEngine] = useState<TranslationEngine>(initialTranslationEngine ?? 'local');
  const [translationSource, setTranslationSource] = useState(initialTranslationSource ?? '');
  const [translationQuality, setTranslationQuality] = useState<LocalTranslationQuality>(initialTranslationQuality ?? 'standard');
  const localRequest = useRef<string | null>(null);
  const [optionsOpen, setOptionsOpen] = useState(true);
  const lastTranslationTarget = useRef(translationTarget || 'English');
  useEffect(() => { if (translationTarget) lastTranslationTarget.current = translationTarget; }, [translationTarget]);
  const [translationPreparing, setTranslationPreparing] = useState(false);
  const [offlineReadiness, setOfflineReadiness] = useState<LocalTranslationReadiness | null>(null);
  const offline = !!translationTarget && translationEngine === 'local';
  const offlineReady = offlineReadiness?.key === localTranslationKey(translationSource, translationTarget ?? '', translationQuality) && offlineReadiness.ready;
  const cacheKey = (id: string) => translationTarget ? JSON.stringify([id, translationEngine, translationSource, translationQuality]) : id;
  const styleChangePending = extraStyleChoice?.pending || draftingChoice?.pending;
  const presentation = stylePackPresentation(pack, t);
  const savedExamples = pack.examples
    .filter(example => example.input.trim() && example.output.trim())
    .map(example => stylePackExamplePresentation(pack, example, t));
  const dialog = useRef<HTMLDialogElement>(null);
  const workspaceScroll = useRef<HTMLDivElement>(null);
  const dialogScroll = useRef(0);
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
  const [previousStep, setPreviousStep] = useState<{ input: string; output: string; name: string; translated: boolean; continued: boolean } | null>(null);
  // Keep the first input through explicit result handoffs. Persist it using the
  // existing immutable draft's original field, so older versions can still read it.
  const [workflowOriginal, setWorkflowOriginal] = useState<string | null>(inputOriginal ?? null);
  const [translationReference, setTranslationReference] = useState<{ input: string; target: string; source: string } | null>(null);
  const [refinementReturn, setRefinementReturn] = useState<{
    input: string; name: string | null; previousStep: typeof previousStep;
    engine: TranslationEngine; source: string; quality: LocalTranslationQuality;
  } | null>(null);
  // Session-only comparisons, never History or persistent customer text storage.
  const completed = useRef(new Map<string, { signature: string; input: string; result: StyleTextTrialResult; edited: string | null }>());
  // A request can finish before its target style becomes selected. Carry that
  // exact fresh result through the selection, rather than calling it a restore
  // merely because it has just been added to the comparison cache.
  const freshSelectionResult = useRef<{ packId: string; signature: string; result: StyleTextTrialResult } | null>(null);
  const signature = JSON.stringify(translationTarget ? [draft, pack.prompt, translationEngine, translationSource, translationQuality] : [draft, pack.prompt]);
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
  const originalForSave = workflowOriginal ?? draft;
  const generatedDraftName = translationTarget && result
    ? withDraftOriginName(translationOriginName, `${presentation.name} · ${t(result.usedTextService ? 'textDraft.routeCloud' : 'textDraft.routeLocal')}${editedOutput !== null ? ` · ${t('textDraft.routeEdited')}` : ''}`)
    : presentation.name;
  const draftName = draftNameEdit ?? generatedDraftName;
  const savedDraftName = draftName.trim();
  const draftNameInvalid = !savedDraftName || Array.from(savedDraftName).length > 80;
  const savedCurrentDraft = savedDraftSignature === JSON.stringify([savedDraftName, originalForSave, resultOutput]);
  const canProcess = offline ? offlineReady : !!readiness?.available && (!readiness.requiresTextService || readiness.textServiceConfigured);
  const cloudRetranslationPrepared = !!translationTarget && translationEngine === 'cloud' &&
    previousStep?.translated && !previousStep.continued && !result && !pending;

  useEffect(() => {
    if (localRequest.current) { void cancelLocalTranslation(localRequest.current).catch(() => {}); localRequest.current = null; }
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
    // Reveal the produced text; result actions have their own non-scrolling
    // layout region, including after naming, saving and window resizing.
    if (result && !restored) {
      resultSection.current?.scrollIntoView({ block: 'start', inline: 'nearest' });
    }
  }, [result, restored]);

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
    if (!modalSuspended) {
      element?.showModal();
      if (workspaceScroll.current) workspaceScroll.current.scrollTop = dialogScroll.current;
    }
    return () => {
      if (element?.open && workspaceScroll.current) dialogScroll.current = workspaceScroll.current.scrollTop;
      element?.close();
    };
  }, [modalSuspended]);

  useEffect(() => {
    return () => {
      generation.current += 1;
      if (localRequest.current) void cancelLocalTranslation(localRequest.current).catch(() => {});
      if (deliveryRequest.current) void cancelStyleTrialInsert(deliveryRequest.current).catch(() => {});
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
      const targetSignature = JSON.stringify(translationTarget ? [draft, target.prompt, translationEngine, translationSource, translationQuality] : [draft, target.prompt]);
      const cached = prepare ? completed.current.get(cacheKey(target.id)) : null;
      const matching = cached?.signature === targetSignature ? cached : null;
      const localId = offline && !matching ? crypto.randomUUID() : undefined;
      if (localId) localRequest.current = localId;
      const returned = matching?.result ?? await (translationTarget
        ? offline ? tryLocalTranslationText(draft, translationSource, translationTarget, translationQuality, localId) : tryTranslationText(draft, translationTarget)
        : tryStylePackText(target.id, draft));
      // The backend receipt identifies the route; workspace identity also binds
      // the captured source/method signature, so late results cannot cross it.
      if (generation.current === request) {
        // This response belongs to this exact current input. Validate before
        // caching/display/save, never by rewriting later manual edits or by
        // secretly sending an earlier workflow original back to a service.
        const baseValue = offline ? { ...returned, packId: target.id, packName: target.name } : returned;
        const review = !matching && returned.usedTextService
          ? translationTarget && localTargetCode(translationTarget) ? preserveTranslatedSourceFacts
            : !translationTarget && target.baseMode !== 'raw' && !isMessageDraftStyle(target) ? preserveBufferedSourceFacts : null
          : null;
        const value = review ? { ...baseValue, outputText: review(draft, returned.outputText).outputText } : baseValue;
        if (translationTarget) setOptionsOpen(false);
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
        if (translationTarget) setOptionsOpen(true);
        const message = String(cause instanceof Error ? cause.message : cause);
        const key = {
          style_trial_native_required: 'nativeRequired',
          style_trial_invalid_text: 'invalidText',
          style_trial_timeout: 'timeout',
        }[message];
        setResult(result); setResultSignature(resultSignature); setEditedOutput(editedOutput);
        setError(message === 'buffered_source_facts_unmatched' ? t('sourceFacts.unmatched')
          : offline ? t(`offlineTranslation.${localTranslationErrorKey(cause)}`) : translationTarget && /status 401|AuthenticationError|Unauthorized|credentials were already rejected/i.test(message) ? t('translation.authFailed')
          : message === 'style_trial_prepare_shorter_failed' ? t('conciseDraft.failed')
          : key ? t(`style.trial.${key}`) : t('style.trial.failed', { error: message }));
      }
    } finally {
      if (generation.current === request) {
        localRequest.current = null;
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

  const continueWithResult = () => {
    if (!onTranslationTargetChange || !result || !resultOutput.trim() || resultOutput.length > 4000 ||
        pending || copying || delivering || draftSaving || styleChangePending || translationPreparing) return;
    // An explicit handoff starts a new input, never a new service request. Keep
    // the exact edited output and its source visible instead of relying on the
    // clipboard or mislabelling the preceding result as a new one.
    setPreviousStep({ input: draft, output: resultOutput, name: savedDraftName || presentation.name, translated: !!translationTarget, continued: true });
    setWorkflowOriginal(current => current ?? draft);
    if (translationTarget) setTranslationReference({ input: draft, target: translationTarget, source: translationSource });
    setRefinementReturn(null);
    if (!translationTarget) {
      setTranslationOriginName(savedDraftName || presentation.name);
      setDraftNameEdit(null);
    } else {
      setDraftNameEdit(savedDraftName || null);
    }
    onDraftChange(resultOutput);
    // The previous source-language choice described the previous input, not
    // this output (which may now be in another language).
    setTranslationSource('');
    if (translationTarget) onRefineTranslation?.();
    onTranslationTargetChange(translationTarget ? '' : lastTranslationTarget.current);
    setOptionsOpen(true);
  };

  const prepareCloudRetranslation = () => {
    if (!offline || !result || pending || copying || delivering || draftSaving || styleChangePending || translationPreparing) return;
    // Keep the local result, including edits, while the original stays as input.
    // Selecting the route cannot send a request or overwrite a saved draft.
    setPreviousStep({ input: draft, output: resultOutput, name: savedDraftName || presentation.name, translated: true, continued: false });
    setDraftNameEdit(null);
    setTranslationEngine('cloud');
    setOptionsOpen(true);
    // Show the selected service settings while keeping explicit submission in
    // the fixed action area. Switching routes must not submit a paid request.
    workspaceScroll.current?.scrollTo({ top: 0 });
  };

  const retranslateBeforeRefinement = () => {
    if (!onTranslationTargetChange || translationTarget || !translationReference ||
        pending || copying || delivering || draftSaving || styleChangePending || translationPreparing) return;
    // Revisit the actual pre-translation input, never send an already mistranslated
    // sentence as a substitute. Switching is preparation, not a paid request.
    setRefinementReturn({ input: draft, name: draftNameEdit, previousStep,
      engine: translationEngine, source: translationSource, quality: translationQuality });
    setPreviousStep({ input: translationReference.input, output: result ? resultOutput : draft,
      name: savedDraftName || presentation.name, translated: true, continued: false });
    onDraftChange(translationReference.input);
    onTranslationTargetChange(translationReference.target);
    setTranslationSource(translationReference.source);
    setTranslationEngine('cloud');
    setDraftNameEdit(null);
    setOptionsOpen(true);
    workspaceScroll.current?.scrollTo({ top: 0 });
  };

  const returnToRefinement = () => {
    if (!onTranslationTargetChange || !refinementReturn || pending || copying || delivering ||
        draftSaving || styleChangePending || translationPreparing) return;
    onDraftChange(refinementReturn.input);
    onTranslationTargetChange('');
    setTranslationEngine(refinementReturn.engine);
    setTranslationSource(refinementReturn.source);
    setTranslationQuality(refinementReturn.quality);
    setDraftNameEdit(refinementReturn.name);
    setPreviousStep(refinementReturn.previousStep);
    setRefinementReturn(null);
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
      aria-labelledby={`${workspaceId}-title`}
      aria-describedby={`${workspaceId}-description`}
      onCancel={event => { event.preventDefault(); if (!draftSaving) onClose(); }}
    >
      <div className="ol-style-trial-header">
        <h2 id={`${workspaceId}-title`} data-style-trial-title style={{ margin: 0, fontSize: 21 }}>{translationTarget ? t('offlineTranslation.title', { language: translationLanguageLabel(translationTarget, i18n.resolvedLanguage ?? i18n.language) }) : onTranslationTargetChange ? t('textWorkspace.title') : t('style.trial.title', { name: presentation.name })}</h2>
        <Btn size="sm" variant="ghost" disabled={draftSaving} onClick={() => { if (!draftSaving) onClose(); }}>{t('common.close')}</Btn>
      </div>
      <div ref={workspaceScroll} className="ol-style-trial-scroll ol-thinscroll" data-workspace-scroll>
      <p id={`${workspaceId}-description`} className="ol-style-trial-hint" data-style-trial-service-notice>{t(offline ? 'offlineTranslation.localNotice' : readiness?.requiresTextService === false ? prepareShorter ? 'textWorkspace.localWithShorterNotice' : 'textWorkspace.localNotice' : 'textWorkspace.cloudNotice')}</p>
      {draftingChoice && <div className="ol-style-trial-drafting-choice" data-drafting-entry>
        <Btn size="sm" variant="ghost" disabled={pending || copying || delivering || draftSaving || translationPreparing || styleChangePending}
          onClick={draftingChoice.onChoose}>{draftingChoice.label}</Btn>
        <p className="ol-style-trial-hint">{draftingChoice.hint}</p>
        {draftingChoice.error && <p role="alert" className="ol-style-trial-hint">{draftingChoice.error}</p>}
      </div>}
      {inputOriginName && <p role="note" className="ol-style-trial-hint" data-saved-draft-origin>{t('textWorkspace.savedDraftInput', { name: inputOriginName })}</p>}
      {!translationTarget && previousStep?.translated && <p role="note" className="ol-style-trial-hint" data-translation-refine-hint>{t('textWorkspace.refineTranslationHint')}</p>}
      {!translationTarget && translationReference && onTranslationTargetChange && <div className="ol-style-trial-hint" data-refinement-original-translation>
        <p>{t('textWorkspace.retranslateBeforeRefinementHint')}</p>
        <Btn size="sm" variant="ghost" disabled={pending || copying || delivering || draftSaving || styleChangePending || translationPreparing}
          onClick={retranslateBeforeRefinement}>{t('textWorkspace.retranslateBeforeRefinement')}</Btn>
      </div>}
      {workflowOriginal !== null && workflowOriginal !== draft && <details className="ol-style-trial-details" data-workspace-original>
        <summary>{t('textWorkspace.workflowOriginal')}</summary>
        <p className="ol-style-trial-hint">{t('textWorkspace.workflowOriginalHint')}</p>
        <pre>{workflowOriginal}</pre>
      </details>}
      <details className="ol-style-trial-options-panel" data-text-options-panel open={optionsOpen}
        onToggle={event => setOptionsOpen(event.currentTarget.open)}>
        <summary>{translationTarget ? t(offline ? 'textWorkspace.translationOptions' : 'textDraft.cloudTranslationOptions', {
          source: translationSource ? translationLanguageLabel(translationSource, i18n.resolvedLanguage ?? i18n.language) : t('offlineTranslation.chooseSource'),
          target: translationLanguageLabel(translationTarget, i18n.resolvedLanguage ?? i18n.language),
          engine: offline ? `${t('offlineTranslation.local')} · ${t(`offlineTranslation.${translationQuality}`)}` : t('offlineTranslation.cloud'),
        }) : t('textWorkspace.styleOptions', { style: presentation.name })}</summary>
      <div className="ol-style-trial-options">
      {onTranslationTargetChange && <label className="ol-style-trial-picker" data-text-task-choice>
        <span>{t('textWorkspace.task')}</span>
        <select aria-label={t('textWorkspace.task')} value={translationTarget ? 'translate' : 'text'}
          disabled={pending || copying || delivering || draftSaving || styleChangePending || translationPreparing}
          onChange={event => onTranslationTargetChange(event.target.value === 'translate' ? lastTranslationTarget.current : '')}>
          <option value="text">{t('textWorkspace.textTask')}</option>
          <option value="translate">{t('textWorkspace.translationTask')}</option>
        </select>
      </label>}
      {onTranslationTargetChange && translationTarget && <label className="ol-style-trial-picker" data-translation-output-choice>
        <span>{t('translation.outputLanguage')}</span>
        <select aria-label={t('translation.outputLanguage')} value={translationTarget ?? ''}
          disabled={pending || copying || delivering || draftSaving || styleChangePending || translationPreparing}
          onChange={event => onTranslationTargetChange(event.target.value)}>
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
        {offline && <LocalTranslationSetup key={translationQuality} quality={translationQuality} onQualityChange={setTranslationQuality} source={translationSource} target={translationTarget}
          onSourceChange={setTranslationSource} disabled={pending || copying || delivering || draftSaving}
          onReadiness={setOfflineReadiness} onBusy={setTranslationPreparing} showNotice={false} />}
      </>}
      {packs && onSelectPack && <label className="ol-style-trial-picker">
        <span>{t('style.trial.styleLabel')}</span>
        <select aria-label={t('style.trial.styleLabel')} value={pack.id} disabled={pending || copying || delivering || draftSaving || styleChangePending}
          onChange={event => onSelectPack(event.target.value)}>
          {packs.map(option => <option key={option.id} value={option.id}>{stylePackPresentation(option, t).name}</option>)}
        </select>
      </label>}
      </div>
      </details>
      {translationTarget && <p className="ol-style-trial-hint" data-translation-trial-scope>{t('offlineTranslation.trialOnly')}</p>}
      {!translationTarget && <p className="ol-style-trial-hint ol-style-trial-purpose" data-style-trial-purpose>{presentation.description}</p>}
      {(!readiness || !readiness.available || (readiness.requiresTextService && !readiness.textServiceConfigured) || readinessError) && <div className="ol-style-trial-availability">
      {readiness?.available === false && <p role="status" className="ol-style-trial-hint">{t('style.trial.nativeRequired')}</p>}
      {readiness?.available && readiness.requiresTextService && !readiness.textServiceConfigured && <p role="status" className="ol-style-trial-hint">{t('style.trial.needsService')}</p>}
      {!readiness && !readinessError && <p role="status" className="ol-style-trial-hint">{t('style.trial.checking')}</p>}
      {readinessError && <p role="alert" className="ol-style-trial-hint">{t('style.trial.readinessFailed')} <Btn size="sm" onClick={() => setReadinessAttempt(value => value + 1)}>{t('common.retry')}</Btn></p>}
      </div>}
      <div className="ol-style-trial-workspace">
      <div className="ol-style-trial-draft">
      <label htmlFor={`${workspaceId}-input`} data-style-trial-input-label style={{ display: 'block', fontWeight: 600, marginBottom: 8 }}>
        {t('style.trial.input')}
      </label>
      <textarea
        id={`${workspaceId}-input`}
        className="ol-style-trial-input ol-thinscroll"
        value={draft}
        maxLength={4000}
        rows={6}
        // Readiness gates submission, not drafting: checking/failure must not discard typing.
        disabled={pending || copying || delivering || draftSaving || styleChangePending}
        placeholder={t('style.trial.placeholder')}
        onChange={event => {
          const value = event.target.value;
          onDraftChange(value); setResult(null); setError(''); setCopiedResult(null); setCopyError(false);
          if (!value.trim()) { setWorkflowOriginal(null); setTranslationReference(null); setPreviousStep(null); setRefinementReturn(null); }
        }}
      />
      {offline && !translationSource && canSuggestChineseSource(draft, translationTarget ?? '') && <div className="ol-style-trial-hint" data-local-source-suggestion>
        <p>{t('textWorkspace.hanSourceHint')}</p>
        <Btn size="sm" variant="ghost" disabled={pending || copying || delivering || draftSaving || translationPreparing}
          onClick={() => setTranslationSource('zh')}>{t('textWorkspace.confirmChinese')}</Btn>
      </div>}
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
          {result.usedTextService && result.outputText === draft && editedOutput === null && <p role="note" className="ol-style-trial-hint">{t('politeEdits.unchangedResult', { seconds: (result.elapsedMs / 1000).toFixed(1) })}</p>}
          <textarea className="ol-style-trial-result-body ol-style-trial-result-editor ol-thinscroll"
            aria-label={t('style.trial.resultInput')} value={resultOutput} rows={8} maxLength={100000}
            disabled={copying || delivering || draftSaving}
            onChange={event => {
              if (draftSaving || copyRunning.current || deliveryRequest.current) return;
              const value = event.target.value;
              updateEditedOutput(value);
            }} />
          <label className="ol-style-trial-draft-name">{t('textDraft.nameLabel')}
            <input type="text" aria-label={t('textDraft.nameLabel')} value={draftName} maxLength={80}
              disabled={copying || delivering || draftSaving}
              onChange={event => setDraftNameEdit(event.target.value)} />
          </label>
          {draftNameInvalid && <p role="alert" className="ol-style-trial-hint">{t('textDraft.nameInvalid')}</p>}
          <div className="ol-style-trial-secondary-actions">
            {(!offline || translationTarget === 'English') && <PoliteEditSuggestions key={`${generation.current}:${result?.packId ?? ''}`} text={resultOutput} originalText={draft} localOnly={offline} cloudConfigured={!offline && readiness?.textServiceConfigured === true}
              disabled={copying || delivering || draftSaving} onApply={updateEditedOutput} />}
          </div>
          {onTranslationTargetChange && translationTarget && <p role="note" className="ol-style-trial-hint" data-translation-refine-hint>{t('textWorkspace.refineTranslationHint')}</p>}
          {onTranslationTargetChange && resultOutput.length > 4000 && <p className="ol-style-trial-hint">{t('textWorkspace.nextInputTooLong')}</p>}
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
      {previousStep && <section className="ol-style-trial-comparisons" aria-label={t('textWorkspace.previousStep')} data-text-previous-step>
        <h3>{t('textWorkspace.previousStep')}</h3>
        <p className="ol-style-trial-hint">{t(previousStep.continued ? 'textWorkspace.continuationHint' : 'textWorkspace.retranslationReferenceHint')}</p>
        <h4>{previousStep.name}</h4><pre>{previousStep.output}</pre>
        <details className="ol-style-trial-details" open={previousStep.translated}><summary>{t('earlierTrial.input')}</summary><pre>{previousStep.input}</pre></details>
      </section>}
      {earlier && (!previousStep || earlier.input !== previousStep.input || (earlier.edited ?? earlier.result.outputText) !== previousStep.output) && <section className="ol-style-trial-comparisons" aria-label={t('earlierTrial.title')} data-style-trial-earlier-result>
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
      </div>
      <div className="ol-style-trial-submit" data-workspace-submit>
        {result && (
          <div className="ol-style-trial-primary-actions ol-style-trial-result-toolbar" role="group" aria-label={t('style.trial.output')} data-workspace-result-actions>
            <Btn size="sm" variant="blue" disabled={copying || delivering || draftSaving || !resultOutput.trim()} onClick={() => void copyResult()}>
              {t(copying ? 'style.trial.copying' : copiedResult === result ? 'style.trial.copied' : 'style.trial.copyResult')}
            </Btn>
            <SaveTextDraft compact name={savedDraftName} original={originalForSave} text={resultOutput} disabled={pending || copying || delivering || draftNameInvalid || styleChangePending || translationPreparing} onBusy={setDraftSaving}
              onOpenSaved={(id,onUnavailable)=>{if(onOpenSavedDraft)onOpenSavedDraft(id,onUnavailable);else setLocalLibrary({id,onUnavailable});}}
              onUnsaved={() => setSavedDraftSignature('')}
              onSaved={text => setSavedDraftSignature(JSON.stringify([savedDraftName, originalForSave, text]))} />
            {onTranslationTargetChange && <Btn size="sm" variant="ghost"
              disabled={pending || copying || delivering || draftSaving || styleChangePending || translationPreparing || !resultOutput.trim() || resultOutput.length > 4000}
              onClick={continueWithResult}>{t(translationTarget ? 'textWorkspace.refineResult' : 'textWorkspace.translateResult')}</Btn>}
            {offline && <Btn size="sm" variant="ghost" data-cloud-retranslation
              disabled={pending || copying || delivering || draftSaving || styleChangePending || translationPreparing}
              onClick={prepareCloudRetranslation}>{t('textWorkspace.retranslateOriginal')}</Btn>}
          </div>
        )}
        {cloudRetranslationPrepared && <p role="status" className="ol-style-trial-hint" data-cloud-retranslation-ready>{t('textWorkspace.retranslationReferenceHint')}</p>}
        <div className="ol-style-trial-submit-actions">
          {refinementReturn && <Btn variant="ghost" disabled={pending || copying || delivering || draftSaving || styleChangePending || translationPreparing}
            onClick={returnToRefinement}>{t('textWorkspace.returnToRefinement')}</Btn>}
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
          {offline && <Btn size="sm" variant="ghost" onClick={() => { if (localRequest.current) void cancelLocalTranslation(localRequest.current).catch(() => {}); }}>{t('common.cancel')}</Btn>}
        </p>}
        {error && <p role="alert" className="ol-style-trial-submit-error">{error}</p>}
      </div>
      {keyPhrase !== null && <DeviceLayoutsDialog rememberSelection initialProfile={writingDeviceKeyProfile()} incomingText={keyPhrase} onLeaveForPage={onClose} onClose={() => setKeyPhrase(null)} />}
    </dialog>
    {providersOpen && <ProviderSettingsDialog kind="text" onClose={() => {
      setProvidersOpen(false); setReadinessAttempt(value => value + 1);
    }} />}
    {localLibrary && <SavedTextDraftsDialog initialDraftId={localLibrary.id} onUnavailable={localLibrary.onUnavailable}
      onClose={()=>setLocalLibrary(null)} />}</>
  );
}
