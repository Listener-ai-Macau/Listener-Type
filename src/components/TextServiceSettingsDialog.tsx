import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { ProvidersSection } from '../pages/settings/ProvidersSection';
import { Btn } from '../pages/_atoms';
import { waitForProviderWrites } from '../lib/providerWriteBarrier';
import './StyleTextTrial.css';

/** A sibling modal: the text workspace and its source transcript stay mounted. */
export function TextServiceSettingsDialog({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation();
  const dialog = useRef<HTMLDialogElement>(null);
  const closing = useRef(false);
  const [saving, setSaving] = useState(false);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    const element = dialog.current;
    element?.showModal();
    return () => element?.close();
  }, []);
  const close = async () => {
    if (closing.current) return;
    closing.current = true;
    // Escape needs the same blur flush as clicking Close, before unmount cancels debounce.
    const active = document.activeElement;
    if (active instanceof HTMLElement && dialog.current?.contains(active)) active.blur();
    setSaving(true); setFailed(false);
    const saved = await waitForProviderWrites();
    if (saved) { onClose(); return; }
    closing.current = false; setSaving(false); setFailed(true);
  };
  return <dialog ref={dialog} className="ol-style-trial" aria-label={t('style.trial.providers')}
    onCancel={event => { event.preventDefault(); void close(); }}>
    <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 16 }}>
      <h2>{t('style.trial.providers')}</h2>
      <Btn variant="ghost" disabled={saving} onClick={() => void close()}>{t(saving ? 'common.saving' : 'common.close')}</Btn>
    </div>
    <p className="ol-style-trial-hint">{t('style.trial.setupReturn')}</p>
    <fieldset disabled={saving} style={{ border: 0, padding: 0, margin: 0, minWidth: 0 }}>
      <ProvidersSection prioritizeTextService textOnly onOpenLocalAsr={() => {}} />
    </fieldset>
    {failed && <div role="alert"><p>{t('style.trial.setupSaveFailed')}</p>
      <Btn variant="ghost" onClick={onClose}>{t('style.trial.setupCloseAnyway')}</Btn>
    </div>}
  </dialog>;
}
