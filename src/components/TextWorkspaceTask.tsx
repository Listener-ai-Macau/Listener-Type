import { useTranslation } from 'react-i18next';

export type TextWorkspaceTaskId = 'text' | 'draft' | 'translate';
export interface TextWorkspaceTaskControl {
  task: TextWorkspaceTaskId;
  draftFormat: 'body' | 'email';
  pending: boolean;
  error: string;
  onChoose: (task: TextWorkspaceTaskId) => void;
  onDraftFormatChange: (format: 'body' | 'email') => void;
}

export function TextWorkspaceTask({ control, disabled }: {
  control: TextWorkspaceTaskControl;
  disabled: boolean;
}) {
  const { t } = useTranslation();
  return <div data-workspace-task>
    <div className="ol-style-trial-options">
      <label className="ol-style-trial-picker">
        <span>{t('simpleTextWorkflow.task')}</span>
        <select aria-label={t('simpleTextWorkflow.task')} value={control.task}
          disabled={disabled || control.pending}
          onChange={event => control.onChoose(event.target.value as TextWorkspaceTaskId)}>
          <option value="text">{t('simpleTextWorkflow.refine')}</option>
          <option value="draft">{t('simpleTextWorkflow.draft')}</option>
          <option value="translate">{t('simpleTextWorkflow.translate')}</option>
        </select>
      </label>
      {control.task === 'draft' && <label className="ol-style-trial-picker">
        <span>{t('simpleTextWorkflow.draftFormat')}</span>
        <select aria-label={t('simpleTextWorkflow.draftFormat')} value={control.draftFormat}
          disabled={disabled || control.pending}
          onChange={event => control.onDraftFormatChange(event.target.value as 'body' | 'email')}>
          <option value="body">{t('simpleTextWorkflow.messageBody')}</option>
          <option value="email">{t('simpleTextWorkflow.email')}</option>
        </select>
      </label>}
    </div>
    {control.pending && <p role="status" className="ol-style-trial-hint">{t('textWorkspace.preparing')}</p>}
    {control.error && <p role="alert" className="ol-style-trial-hint">{control.error}</p>}
  </div>;
}
