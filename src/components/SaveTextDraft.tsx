import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { isTauri } from '../lib/ipc';
import { useTextDraftSave } from '../lib/useTextDraftSave';
import { Btn } from '../pages/_atoms';
import { SavedTextDraftsDialog } from './SavedTextDraftsDialog';

export function SaveTextDraft({ name, original, text, disabled, onBusy, onSaved, onUnsaved, onOpenSaved, compact = false }: {
  name: string; original: string; text: string; disabled: boolean; onBusy: (busy: boolean) => void;
  onSaved?: (text: string) => void;
  onUnsaved?: () => void;
  onOpenSaved?: (id: string, onUnavailable: (id: string) => void) => void;
  compact?: boolean;
}) {
  const { t } = useTranslation();
  const { save, saving, isSaved, savedId, error, invalidate } = useTextDraftSave({ name, original, text, disabled, onBusy, onSaved, onUnsaved });
  const [draftsOpen,setDraftsOpen] = useState(false);
  return <><div className="ol-save-text-draft">
    <Btn size="sm" variant="ghost" disabled={disabled||saving||!text.trim()||isSaved} onClick={()=>void save()}>
      {t(saving?'textDraft.saving':isSaved?'textDraft.saved':'textDraft.save')}
    </Btn>
    {isSaved&&<span role="status" className="ol-style-trial-hint"> {t(isTauri?compact?'textWorkspace.savedWhere':'textDraft.savedWhere':'textDraft.browserSaved')}</span>}
    {isSaved&&<Btn size="sm" variant="ghost" disabled={disabled||saving}
      onClick={()=>{if(onOpenSaved&&savedId)onOpenSaved(savedId,invalidate);else setDraftsOpen(true);}}>{t('textDraft.openSaved')}</Btn>}
    {error&&<p role="alert" className="ol-style-trial-hint">{error}</p>}
  </div>{draftsOpen&&<SavedTextDraftsDialog initialDraftId={savedId} onClose={()=>setDraftsOpen(false)}
    onUnavailable={invalidate} />}</>;
}
