import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { createStylePackFromTemplate, listStylePacks } from '../lib/ipc';
import { everydayListStyle, isEverydayListStyle } from '../lib/everydayListStyle';
import { conciseDraftStyle, isConciseDraftStyle } from '../lib/conciseDraftStyle';
import { emailDraftStyle, isEmailDraftStyle, isMessageBodyDraftStyle, isMessageDraftStyle, messageDraftStyle } from '../lib/messageDraftStyle';
import type { StylePack } from '../lib/types';
import { Btn } from '../pages/_atoms';
import { StyleTextTrial } from './StyleTextTrial';
import { translationTrialPack } from '../lib/translationTrial';
import { SavedTextDraftsDialog } from './SavedTextDraftsDialog';

/** A text workspace, not a style activation or a shortcut to pack management. */
export function QuickStyleTextTrial({ draft, onDraftChange, onClose, initialTranslationTarget = '', inputOriginName, inputOriginal }: {
  draft: string;
  onDraftChange: (text: string) => void;
  onClose: () => void;
  initialTranslationTarget?: string;
  inputOriginName?: string;
  inputOriginal?: string;
}) {
  const { t } = useTranslation();
  const [packs, setPacks] = useState<StylePack[]>([]);
  const [selectedId, setSelectedId] = useState('');
  const [translationTarget, setTranslationTarget] = useState(initialTranslationTarget);
  const [originName, setOriginName] = useState(inputOriginName);
  const [originalInput, setOriginalInput] = useState(inputOriginal);
  const [library, setLibrary] = useState<{ id: string; onUnavailable: (id: string) => void } | null>(null);
  const [workspaceRevision, setWorkspaceRevision] = useState(0);
  const [failed, setFailed] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const [adding, setAdding] = useState<'list' | 'concise' | 'draft' | 'email' | null>(null);
  const [addFailed, setAddFailed] = useState<'list' | 'concise' | 'draft' | 'email' | null>(null);
  const addingRef = useRef(false);
  const mounted = useRef(false);
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);
  useEffect(() => {
    let current = true;
    setFailed(false);
    void listStylePacks().then(value => {
      if (!current) return;
      if (!value.length) { setFailed(true); return; }
      setPacks(value);
      // Read the saved active style for a starting selection; never activate it.
      setSelectedId((value.find(pack => pack.active) ?? value.find(pack => pack.id === 'builtin.raw') ?? value[0]).id);
    }).catch(() => { if (current) setFailed(true); });
    return () => { current = false; };
  }, [attempt]);
  const selected = packs.find(pack => pack.id === selectedId);
  const savedEverydayList = packs.find(isEverydayListStyle);
  const savedConcise = packs.find(isConciseDraftStyle);
  const savedMessageDraft = packs.find(isMessageBodyDraftStyle);
  const savedEmailDraft = packs.find(isEmailDraftStyle);
  const prepareConcise = async (): Promise<StylePack> => {
    if (savedConcise) return savedConcise;
    if (addingRef.current) throw new Error('style_trial_prepare_shorter_failed');
    addingRef.current = true;
    setAdding('concise');
    setAddFailed(null);
    try {
      const created = await createStylePackFromTemplate(conciseDraftStyle(t), true);
      if (mounted.current) setPacks(current => [...current.filter(pack => pack.id !== created.id), created]);
      return created;
    } catch {
      throw new Error('style_trial_prepare_shorter_failed');
    } finally {
      addingRef.current = false;
      if (mounted.current) setAdding(null);
    }
  };
  const chooseMessageDraft = async () => {
    if (addingRef.current) return;
    if (savedMessageDraft) { setSelectedId(savedMessageDraft.id); return; }
    addingRef.current = true;
    setAdding('draft');
    setAddFailed(null);
    try {
      const created = await createStylePackFromTemplate(messageDraftStyle(t), true);
      if (mounted.current) {
        setPacks(current => [...current.filter(pack => pack.id !== created.id), created]);
        setSelectedId(created.id);
      }
    } catch {
      if (mounted.current) setAddFailed('draft');
    } finally {
      addingRef.current = false;
      if (mounted.current) setAdding(null);
    }
  };
  const chooseEmailDraft = async () => {
    if (addingRef.current) return;
    if (savedEmailDraft) { setSelectedId(savedEmailDraft.id); return; }
    addingRef.current = true; setAdding('email'); setAddFailed(null);
    try {
      const created = await createStylePackFromTemplate(emailDraftStyle(t), true);
      if (mounted.current) {
        setPacks(current => [...current.filter(pack => pack.id !== created.id), created]);
        setSelectedId(created.id);
      }
    } catch { if (mounted.current) setAddFailed('email'); }
    finally { addingRef.current = false; if (mounted.current) setAdding(null); }
  };
  const chooseEverydayList = async () => {
    if (addingRef.current) return;
    if (savedEverydayList) { setSelectedId(savedEverydayList.id); return; }
    addingRef.current = true;
    setAdding('list');
    setAddFailed(null);
    try {
      const created = await createStylePackFromTemplate(everydayListStyle(t), true);
      if (mounted.current) {
        setPacks(current => [...current.filter(pack => pack.id !== created.id), created]);
        setSelectedId(created.id);
      }
    } catch {
      if (mounted.current) setAddFailed('list');
    } finally {
      addingRef.current = false;
      if (mounted.current) setAdding(null);
    }
  };
  const displayed = translationTarget ? translationTrialPack(translationTarget,
    t('translation.trialName', { language: translationTarget }), t('translation.tryTextHint')) : selected;
  return <>{displayed ? <StyleTextTrial key={workspaceRevision} pack={displayed} packs={translationTarget ? undefined : packs}
    onSelectPack={translationTarget ? undefined : setSelectedId} draft={draft} onDraftChange={onDraftChange}
    translationTarget={translationTarget || undefined} onTranslationTargetChange={setTranslationTarget}
    onRefineTranslation={selected && isMessageDraftStyle(selected) ? () => {
      // The next input is an existing translated body, not writing directions.
      // Change only this workspace's selection; retain other chosen styles.
      const refinement = packs.find(pack => pack.id === 'builtin.light');
      if (refinement) setSelectedId(refinement.id);
    } : undefined}
    draftingChoice={!translationTarget && selected && !isMessageBodyDraftStyle(selected) ? {
      label: t(savedMessageDraft ? 'messageDraft.choose' : 'messageDraft.add'),
      hint: t('messageDraft.hint'), pending: adding !== null,
      error: addFailed === 'draft' ? t('messageDraft.failed') : '', onChoose: () => void chooseMessageDraft(),
    } : undefined}
    emailDraftingChoice={!translationTarget && selected && !isEmailDraftStyle(selected) ? {
      label: t(savedEmailDraft ? 'messageDraft.emailChoose' : 'messageDraft.emailAdd'),
      hint: t('messageDraft.emailHint'), pending: adding !== null,
      error: addFailed === 'email' ? t('messageDraft.emailFailed') : '', onChoose: () => void chooseEmailDraft(),
    } : undefined}
    extraStyleChoice={!translationTarget && selected?.id === 'builtin.structured' ? {
      label: t(savedEverydayList ? 'style.everydayList.choose' : 'style.everydayList.add'),
      hint: t('style.everydayList.hint'), pending: adding !== null,
      error: addFailed === 'list' ? t('style.everydayList.failed') : '', onChoose: () => void chooseEverydayList(),
    } : undefined}
    prepareShorter={translationTarget ? undefined : prepareConcise}
    inputOriginName={originName} inputOriginal={originalInput} suspended={library !== null} onOpenSavedDraft={(id,onUnavailable)=>setLibrary({id,onUnavailable})} onClose={onClose} /> :
    <TrialLoading failed={failed} onRetry={() => setAttempt(value => value + 1)} onClose={onClose} title={t('overview.tryTypedText')} />}
    {library !== null && <SavedTextDraftsDialog initialDraftId={library.id} onUnavailable={library.onUnavailable}
      onClose={() => setLibrary(null)} onContinueDraft={value => {
        setOriginName(value.name); setOriginalInput(value.original); onDraftChange(value.text); setTranslationTarget('');
        setWorkspaceRevision(current => current + 1); setLibrary(null);
      }} />}
  </>;
}

function TrialLoading({ failed, onRetry, onClose, title }: {
  failed: boolean; onRetry: () => void; onClose: () => void; title: string;
}) {
  const { t } = useTranslation();
  const dialog = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    const element = dialog.current;
    element?.showModal();
    return () => element?.close();
  }, []);
  return <dialog ref={dialog} className="ol-style-trial" aria-label={title}
    onCancel={event => { event.preventDefault(); onClose(); }}>
    <h2>{title}</h2>
    <p role={failed ? 'alert' : 'status'}>{t(failed ? 'style.trial.stylesFailed' : 'style.trial.checking')}</p>
    {failed && <Btn onClick={onRetry}>{t('common.retry')}</Btn>}
    <Btn variant="ghost" onClick={onClose}>{t('common.close')}</Btn>
  </dialog>;
}
