import { useTranslation } from 'react-i18next';
import { type SavedTextDraft, isTauri } from '../lib/ipc';
import { useTextDraftSave } from '../lib/useTextDraftSave';
import { Btn } from '../pages/_atoms';

export function SavedTextDraftCopyEditor({ record, name, text, disabled, onNameChange, onTextChange, onBusy, onCreated, onBack }: {
  record: SavedTextDraft; name: string; text: string; disabled: boolean;
  onNameChange: (name: string) => void; onTextChange: (text: string) => void;
  onBusy: (busy: boolean) => void; onCreated: (record: SavedTextDraft) => void; onBack: () => void;
}) {
  const { t } = useTranslation();
  const validName = !!name.trim() && Array.from(name.trim()).length <= 80;
  const saved = useTextDraftSave({ name: name.trim(), original: record.original, text, disabled: disabled || !validName, onBusy, onSavedRecord: onCreated });
  return <section aria-label={t('textDraft.copyEditorTitle')} data-saved-draft-copy-editor>
    <h3>{t('textDraft.copyEditorTitle')}</h3>
    <p className="ol-device-layout-hint">{t('textDraft.copyEditorHint')}</p>
    <label>{t('textDraft.copyNameLabel')}<input type="text" aria-label={t('textDraft.copyNameLabel')} value={name} maxLength={80}
      disabled={disabled || saved.saving} onChange={event => onNameChange(event.target.value)} style={{ width: '100%' }} /></label>
    {!validName && <p role="alert">{t('textDraft.nameInvalid')}</p>}
    <textarea aria-label={t('textDraft.copyEditorText')} value={text} maxLength={100000} rows={9}
      disabled={disabled || saved.saving} onChange={event => onTextChange(event.target.value)}
      style={{ width: '100%', lineHeight: 1.6, resize: 'vertical', marginTop: 8 }} />
    <details style={{ margin: '14px 0' }}><summary>{t('textDraft.original')}</summary><pre className="ol-saved-phrase-text">{record.original}</pre></details>
    <div style={{ display: 'flex', gap: 10, flexWrap: 'wrap' }}>
      <Btn size="sm" disabled={disabled || saved.saving || saved.isSaved || !validName || !text.trim()} onClick={() => void saved.save()}>
        {t(saved.saving ? 'textDraft.saving' : saved.isSaved ? 'textDraft.copySaved' : 'textDraft.saveCopy')}
      </Btn>
      <Btn size="sm" variant="ghost" disabled={disabled || saved.saving} onClick={onBack}>{t('textDraft.backToDrafts')}</Btn>
    </div>
    {saved.isSaved && <p role="status" className="ol-device-layout-hint">{t(isTauri ? 'textWorkspace.savedWhere' : 'textDraft.browserSaved')}</p>}
    {saved.error && <p role="alert">{saved.error}</p>}
  </section>;
}
