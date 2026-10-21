import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { createStylePackFromTemplate, listStylePacks } from '../lib/ipc';
import { everydayListStyle, isEverydayListStyle } from '../lib/everydayListStyle';
import { conciseDraftStyle, isConciseDraftStyle } from '../lib/conciseDraftStyle';
import type { StylePack } from '../lib/types';
import { Btn } from '../pages/_atoms';
import { StyleTextTrial } from './StyleTextTrial';
import { TextServiceSettingsDialog } from './TextServiceSettingsDialog';

/** A text workspace, not a style activation or a shortcut to pack management. */
export function QuickStyleTextTrial({ draft, onDraftChange, onClose, onOpenProviders }: {
  draft: string;
  onDraftChange: (text: string) => void;
  onClose: () => void;
  onOpenProviders?: () => void;
}) {
  const { t } = useTranslation();
  const [packs, setPacks] = useState<StylePack[]>([]);
  const [selectedId, setSelectedId] = useState('');
  const [failed, setFailed] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const [adding, setAdding] = useState<'list' | 'concise' | null>(null);
  const [addFailed, setAddFailed] = useState<'list' | 'concise' | null>(null);
  const [providersOpen, setProvidersOpen] = useState(false);
  const [readinessRevision, setReadinessRevision] = useState(0);
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
  const chooseTemplate = async (kind: 'list' | 'concise') => {
    if (addingRef.current) return;
    const saved = kind === 'list' ? savedEverydayList : savedConcise;
    if (saved) { setSelectedId(saved.id); return; }
    addingRef.current = true;
    setAdding(kind);
    setAddFailed(null);
    try {
      const created = await createStylePackFromTemplate(kind === 'list' ? everydayListStyle(t) : conciseDraftStyle(t), true);
      if (mounted.current) {
        setPacks(current => [...current.filter(pack => pack.id !== created.id), created]);
        setSelectedId(created.id);
      }
    } catch {
      if (mounted.current) setAddFailed(kind);
    } finally {
      addingRef.current = false;
      if (mounted.current) setAdding(null);
    }
  };
  return <>{selected ? <StyleTextTrial pack={selected} packs={packs}
    onSelectPack={setSelectedId} draft={draft} onDraftChange={onDraftChange}
    extraStyleChoice={selected.id === 'builtin.structured' ? {
      label: t(savedEverydayList ? 'style.everydayList.choose' : 'style.everydayList.add'),
      hint: t('style.everydayList.hint'), pending: adding !== null,
      error: addFailed === 'list' ? t('style.everydayList.failed') : '', onChoose: () => void chooseTemplate('list'),
    } : undefined}
    presetStyleChoice={!isConciseDraftStyle(selected) ? {
      label: t(adding === 'concise' ? 'conciseDraft.adding' : savedConcise ? 'conciseDraft.choose' : 'conciseDraft.add'),
      hint: t('conciseDraft.hint'), pending: adding !== null,
      error: addFailed === 'concise' ? t('conciseDraft.failed') : '', onChoose: () => void chooseTemplate('concise'),
    } : undefined}
    onClose={onClose} onOpenProviders={onOpenProviders ?? (() => setProvidersOpen(true))}
    readinessRevision={readinessRevision} /> :
    <TrialLoading failed={failed} onRetry={() => setAttempt(value => value + 1)} onClose={onClose} title={t('overview.tryTypedText')} />}
    {providersOpen && <TextServiceSettingsDialog onClose={() => {
      setProvidersOpen(false); setReadinessRevision(value => value + 1);
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
