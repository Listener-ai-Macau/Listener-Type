import { useTranslation } from 'react-i18next';
import { isTauri, saveQuestionAnswerDraft } from '../lib/ipc';
import { useTextDraftSave } from '../lib/useTextDraftSave';
import { Btn } from '../pages/_atoms';

const noBusy = () => undefined;
const saveAnswer = (id: string, name: string, _original: string, text: string) =>
  saveQuestionAnswerDraft(id, name, text);

/** A click saves this completed answer locally; no clipboard or service call.
 * Keep naming/editing in the ordinary main window, not in the nonactivating
 * question card where a text field could send keyboard input to another app.
 */
export function QaAnswerDraftSave({ question, answer }: { question: string; answer: string }) {
  const { t } = useTranslation();
  const name = Array.from(question.replace(/\s+/g, ' ').trim()).slice(0, 60).join('')
    || t('selectionAsk.title');
  // The draft's original is the answer before future edits, not a possibly
  // private selection embedded in the question's context.
  const saved = useTextDraftSave({ name, original: answer, text: answer,
    disabled: !answer.trim(), onBusy: noBusy, write: saveAnswer });
  return <div data-qa-save-draft style={{ display: 'grid', gap: 4, justifyItems: 'start' }}>
    <Btn size="sm" variant="ghost" disabled={saved.saving || saved.isSaved || !answer.trim()}
      onClick={() => void saved.save()}>
      {t(saved.saving ? 'textDraft.saving' : saved.isSaved ? 'textDraft.saved' : 'textDraft.save')}
    </Btn>
    {saved.isSaved && <span role="status" style={{ fontSize: 11, lineHeight: 1.5, color: 'var(--ol-ink-3)' }}>
      {t(isTauri ? 'textWorkspace.savedWhere' : 'textDraft.browserSaved')}
    </span>}
    {saved.error && <span role="alert" style={{ fontSize: 11, color: 'var(--ol-err)' }}>{saved.error}</span>}
  </div>;
}
