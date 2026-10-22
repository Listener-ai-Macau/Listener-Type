import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { ProvidersSection } from '../pages/settings/ProvidersSection';
import { Btn } from '../pages/_atoms';
import { providerWriteRevision, waitForProviderWrites } from '../lib/providerWriteBarrier';
import './StyleTextTrial.css';

/** A sibling modal: configuring a service never replaces its originating work. */
export function ProviderSettingsDialog({ kind, onClose }: { kind: 'text' | 'speech'; onClose: () => void }) {
  const { t } = useTranslation();
  const title = t(kind === 'text' ? 'style.trial.providers' : 'audioFile.settings');
  const copy = kind === 'text' ? 'style.trial' : 'audioFile';
  const dialog = useRef<HTMLDialogElement>(null);
  const closing = useRef(false);
  const openedAt = useRef(providerWriteRevision());
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
    const saved = await waitForProviderWrites(openedAt.current);
    if (saved) { onClose(); return; }
    closing.current = false; setSaving(false); setFailed(true);
  };
  return <dialog ref={dialog} className="ol-style-trial" aria-label={title}
    onCancel={event => { event.preventDefault(); void close(); }}>
    <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 16 }}>
      <h2>{title}</h2>
      <Btn variant="ghost" disabled={saving} onClick={() => void close()}>{t(saving ? 'common.saving' : 'common.close')}</Btn>
    </div>
    <p className="ol-style-trial-hint">{t(`${copy}.setupReturn`)}</p>
    <fieldset disabled={saving} style={{ border: 0, padding: 0, margin: 0, minWidth: 0 }}>
      <ProvidersSection prioritizeTextService={kind === 'text'} textOnly={kind === 'text'} speechOnly={kind === 'speech'} onOpenLocalAsr={() => {}} />
    </fieldset>
    {failed && <div role="alert"><p>{t(`${copy}.setupSaveFailed`)}</p>
      <Btn variant="ghost" onClick={onClose}>{t(`${copy}.setupCloseAnyway`)}</Btn>
    </div>}
  </dialog>;
}
