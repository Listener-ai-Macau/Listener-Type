import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { isTauri, saveTextDraft } from '../lib/ipc';
import { Btn } from '../pages/_atoms';
import { SavedTextDraftsDialog } from './SavedTextDraftsDialog';

export function SaveTextDraft({ name, original, text, disabled, onBusy, onSaved, compact = false }: {
  name: string; original: string; text: string; disabled: boolean; onBusy: (busy: boolean) => void;
  onSaved?: (text: string) => void;
  compact?: boolean;
}) {
  const { t } = useTranslation();
  const signature = JSON.stringify([name,original,text]);
  const request = useRef<{signature:string;id:string}|null>(null);
  const running = useRef(false), alive = useRef(true);
  const [saving,setSaving] = useState(false), [saved,setSaved] = useState(''), [error,setError] = useState('');
  const [savedId,setSavedId] = useState(''), [draftsOpen,setDraftsOpen] = useState(false);
  useEffect(()=>{alive.current=true;return()=>{alive.current=false;onBusy(false);};},[]);
  useEffect(()=>{setError('');},[signature]);
  const save = async () => {
    if (running.current || disabled || !text.trim() || saved===signature) return;
    if (request.current?.signature!==signature) request.current={signature,id:crypto.randomUUID()};
    running.current=true;setSaving(true);setError('');onBusy(true);
    try {
      const value = await saveTextDraft(request.current.id,Array.from(name).slice(0,80).join(''),original,text);
      if(alive.current){setSaved(signature);setSavedId(value.id);onSaved?.(text);}
    } catch(cause) {
      if(alive.current)setError(t(String(cause).includes('saved_draft_limit')?'textDraft.full':'textDraft.failed'));
    } finally {
      running.current=false;
      if(alive.current){setSaving(false);onBusy(false);}
    }
  };
  return <><div className="ol-save-text-draft">
    <Btn size="sm" variant="ghost" disabled={disabled||saving||!text.trim()||saved===signature} onClick={()=>void save()}>
      {t(saving?'textDraft.saving':saved===signature?'textDraft.saved':'textDraft.save')}
    </Btn>
    {saved===signature&&<span role="status" className="ol-style-trial-hint"> {t(isTauri?compact?'textWorkspace.savedWhere':'textDraft.savedWhere':'textDraft.browserSaved')}</span>}
    {saved===signature&&<Btn size="sm" variant="ghost" disabled={saving} data-open-just-saved-draft
      onClick={()=>setDraftsOpen(true)}>{t('textDraft.openSaved')}</Btn>}
    {error&&<p role="alert" className="ol-style-trial-hint">{error}</p>}
  </div>{draftsOpen&&<SavedTextDraftsDialog initialDraftId={savedId} onClose={()=>setDraftsOpen(false)} />}</>;
}
