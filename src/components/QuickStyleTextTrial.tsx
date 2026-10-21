import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { listStylePacks } from '../lib/ipc';
import type { StylePack } from '../lib/types';
import { Btn } from '../pages/_atoms';
import { StyleTextTrial } from './StyleTextTrial';

/** A text workspace, not a style activation or a shortcut to pack management. */
export function QuickStyleTextTrial({ draft, onDraftChange, onClose, onOpenProviders }: {
  draft: string;
  onDraftChange: (text: string) => void;
  onClose: () => void;
  onOpenProviders: () => void;
}) {
  const { t } = useTranslation();
  const [packs, setPacks] = useState<StylePack[]>([]);
  const [selectedId, setSelectedId] = useState('');
  const [failed, setFailed] = useState(false);
  const [attempt, setAttempt] = useState(0);
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
  return selected ? <StyleTextTrial key={selected.id} pack={selected} packs={packs}
    onSelectPack={setSelectedId} draft={draft} onDraftChange={onDraftChange}
    onClose={onClose} onOpenProviders={onOpenProviders} /> :
    <TrialLoading failed={failed} onRetry={() => setAttempt(value => value + 1)} onClose={onClose} title={t('overview.tryTypedText')} />;
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
